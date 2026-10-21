use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use ad_stream_protocol::InputMessage;
use agent_encoder::{EncodedPacket, EncoderConfig, EncoderError, EncoderPipeline};
use agent_input::{InputConfig, InputError};
use agent_wayland::compositor::{
    ApplicationSpec, CapturedClipboard, ClipboardSelection, CompositorConfig, CompositorError,
    CompositorEvent, HeadlessCompositor,
};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, watch, Notify};
use tokio::task::JoinHandle;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Starting,
    Active,
    Detached,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Debug)]
pub struct SupervisorConfig {
    pub session_id: Uuid,
    pub compositor: CompositorConfig,
    pub input: InputConfig,
    pub encoder: EncoderConfig,
    pub application: ApplicationSpec,
}

enum MediaCommand {
    Lossless(bool),
    Pause,
    Resume,
    Stop,
}

enum CaptureCommand {
    Resize {
        width: u32,
        height: u32,
    },
    Input(InputMessage),
    Clipboard(ClipboardSelection),
    KeyboardLayout {
        layout: String,
        variant: String,
    },
    MonitorLayout {
        properties: Vec<ad_stream_protocol::MonitorProperties>,
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
    },
    Stop,
}

pub struct AgentSupervisor {
    session_id: Uuid,
    state: Arc<RwLock<SessionState>>,
    clients: Arc<Mutex<HashSet<Uuid>>>,
    media_tx: mpsc::Sender<MediaCommand>,
    refresh: Arc<Notify>,
    lossless_refresh: watch::Sender<u64>,
    captured_frames: watch::Receiver<Option<Arc<agent_wayland::damage::CapturedFrame>>>,
    monitors: watch::Sender<Vec<ad_stream_protocol::Rectangle>>,
    rate: Mutex<MediaRate>,
    rate_tx: watch::Sender<(u32, u32)>,
    capture_tx: mpsc::Sender<CaptureCommand>,
    packets: broadcast::Sender<EncodedPacket>,
    clipboards: broadcast::Sender<CapturedClipboard>,
    capture_task: JoinHandle<()>,
    media_task: JoinHandle<()>,
}

impl AgentSupervisor {
    pub async fn start(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        let session_id = config.session_id;
        let state = Arc::new(RwLock::new(SessionState::Starting));
        let clients = Arc::new(Mutex::new(HashSet::new()));
        config.input.validate()?;

        let mut compositor = tokio::task::spawn_blocking({
            let compositor_config = config.compositor;
            move || HeadlessCompositor::spawn(compositor_config)
        })
        .await
        .map_err(SupervisorError::TaskJoin)??;
        compositor.launch(config.application)?;

        let mut rate = MediaRate::new(config.encoder.bitrate_kbps, config.encoder.framerate);
        let size = (config.encoder.width, config.encoder.height);
        let encoder = EncoderPipeline::new(config.encoder)?;
        rate.resize(size.0, size.1);
        let (rate_tx, rate_rx) = watch::channel((rate.bitrate, rate.fps));
        let (media_tx, media_rx) = mpsc::channel(3);
        let refresh = Arc::new(Notify::new());
        let (frames_tx, frames_rx) = watch::channel(None);
        let captured_frames = frames_rx.clone();
        let (lossless_refresh, _) = watch::channel(0_u64);
        let (monitors, _) = watch::channel(vec![ad_stream_protocol::Rectangle {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        }]);
        let (capture_tx, capture_rx) = mpsc::channel(8);
        // A slightly deeper media queue keeps encode bursts from starving the
        // writer at moderate RTT without hiding sustained overload.
        let (packets, _) = broadcast::channel(8);
        let (clipboards, _) = broadcast::channel(16);

        let media_state = state.clone();
        let packet_output = packets.clone();
        let media_refresh = refresh.clone();
        let media_task = tokio::spawn(async move {
            if let Err(error) = run_media_loop(
                encoder,
                media_rx,
                frames_rx,
                packet_output,
                media_refresh,
                rate_rx,
            )
            .await
            {
                error!(%error, "ARPP media pipeline failed");
                set_state(&media_state, SessionState::Failed);
            }
        });

        let capture_state = state.clone();
        let capture_clients = clients.clone();
        let clipboard_output = clipboards.clone();
        let capture_rate = rate_tx.subscribe();
        let capture_task = tokio::spawn(async move {
            if let Err(error) = run_capture_loop(
                &mut compositor,
                capture_rx,
                frames_tx,
                clipboard_output,
                capture_clients,
                capture_rate,
            )
            .await
            {
                error!(%error, "headless compositor capture loop failed");
                set_state(&capture_state, SessionState::Failed);
            }
        });

        set_state(&state, SessionState::Detached);
        info!(%session_id, "persistent AetherDesk session started");
        Ok(Self {
            session_id,
            state,
            clients,
            media_tx,
            refresh,
            lossless_refresh,
            captured_frames,
            monitors,
            rate: Mutex::new(rate),
            rate_tx,
            capture_tx,
            packets,
            clipboards,
            capture_task,
            media_task,
        })
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn state(&self) -> SessionState {
        self.state
            .read()
            .map(|state| *state)
            .unwrap_or(SessionState::Failed)
    }

    pub(crate) fn lossless_source(&self) -> crate::lossless::Source {
        crate::lossless::Source {
            frames: self.captured_frames.clone(),
            monitors: self.monitors.subscribe(),
            rate: self.rate_tx.subscribe(),
            refresh: self.lossless_refresh.subscribe(),
        }
    }

    pub(crate) async fn set_lossless_mode(&self, enabled: bool) -> Result<(), SupervisorError> {
        self.media_tx
            .send(MediaCommand::Lossless(enabled))
            .await
            .map_err(|_| SupervisorError::Stopped)
    }

    pub fn subscribe_packets(&self) -> broadcast::Receiver<EncodedPacket> {
        self.packets.subscribe()
    }

    pub fn subscribe_clipboards(&self) -> broadcast::Receiver<CapturedClipboard> {
        self.clipboards.subscribe()
    }

    pub async fn attach_client(&self, client_id: Uuid) -> Result<(), SupervisorError> {
        self.ensure_running()?;
        let should_resume = {
            let mut clients = self.clients.lock().map_err(|_| SupervisorError::Poisoned)?;
            let was_empty = clients.is_empty();
            let inserted = clients.insert(client_id);
            inserted && was_empty
        };
        if should_resume {
            // A retained desktop may carry quality reductions from an old
            // connection. Start the new connection with a fresh video budget.
            if let Ok(mut rate) = self.rate.lock() {
                rate.reset();
                self.rate_tx.send_replace((rate.bitrate, rate.fps));
            }
            self.media_tx
                .send(MediaCommand::Resume)
                .await
                .map_err(|_| SupervisorError::Stopped)?;
        }
        self.request_refresh().await?;
        set_state_if_running(&self.state, SessionState::Active);
        info!(%client_id, session_id = %self.session_id, "ARPP client attached");
        Ok(())
    }

    pub async fn request_refresh(&self) -> Result<(), SupervisorError> {
        self.ensure_running()?;
        self.refresh.notify_one();
        self.lossless_refresh
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        Ok(())
    }

    pub fn report_delivery(&self, elapsed: Duration, lagged: bool) {
        if let Ok(mut rate) = self.rate.lock() {
            if rate.observe(elapsed, lagged, Instant::now()) {
                self.rate_tx.send_replace((rate.bitrate, rate.fps));
            }
        }
    }

    pub async fn detach_client(&self, client_id: Uuid) -> Result<(), SupervisorError> {
        let should_pause = {
            let mut clients = self.clients.lock().map_err(|_| SupervisorError::Poisoned)?;
            let removed = clients.remove(&client_id);
            removed && clients.is_empty()
        };
        if should_pause {
            self.media_tx
                .send(MediaCommand::Pause)
                .await
                .map_err(|_| SupervisorError::Stopped)?;
            set_state_if_running(&self.state, SessionState::Detached);
        }
        info!(%client_id, session_id = %self.session_id, "ARPP client detached; session retained");
        Ok(())
    }

    pub async fn inject_input(&self, input: &InputMessage) -> Result<(), SupervisorError> {
        if self.state() != SessionState::Active {
            return Err(SupervisorError::NotAttached);
        }
        self.capture_tx
            .send(CaptureCommand::Input(input.clone()))
            .await
            .map_err(|_| SupervisorError::Stopped)
    }

    fn update_video_size(&self, width: u32, height: u32) {
        if let Ok(mut rate) = self.rate.lock() {
            if rate.resize(width, height) {
                self.rate_tx.send_replace((rate.bitrate, rate.fps));
            }
        }
    }

    pub async fn resize(&self, width: u32, height: u32) -> Result<(), SupervisorError> {
        self.update_video_size(width, height);
        self.capture_tx
            .send(CaptureCommand::Resize { width, height })
            .await
            .map_err(|_| SupervisorError::Stopped)?;
        self.monitors
            .send_replace(vec![ad_stream_protocol::Rectangle {
                x: 0,
                y: 0,
                width,
                height,
            }]);
        Ok(())
    }

    pub async fn set_keyboard_layout(
        &self,
        layout: String,
        variant: String,
    ) -> Result<(), SupervisorError> {
        self.capture_tx
            .send(CaptureCommand::KeyboardLayout { layout, variant })
            .await
            .map_err(|_| SupervisorError::Stopped)
    }
    pub async fn set_monitors(
        &self,
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
    ) -> Result<(), SupervisorError> {
        self.set_monitor_configuration(width, height, monitors, vec![])
            .await
    }
    pub async fn set_monitor_configuration(
        &self,
        width: u32,
        height: u32,
        monitors: Vec<ad_stream_protocol::Rectangle>,
        properties: Vec<ad_stream_protocol::MonitorProperties>,
    ) -> Result<(), SupervisorError> {
        self.update_video_size(width, height);
        self.capture_tx
            .send(CaptureCommand::MonitorLayout {
                properties,
                width,
                height,
                monitors: monitors.clone(),
            })
            .await
            .map_err(|_| SupervisorError::Stopped)?;
        if *self.monitors.borrow() != monitors {
            self.monitors.send_replace(monitors);
        }
        Ok(())
    }

    pub async fn set_clipboard(
        &self,
        mime_type: String,
        data: Vec<u8>,
    ) -> Result<(), SupervisorError> {
        let selection = ClipboardSelection::new(mime_type, data)?;
        self.capture_tx
            .send(CaptureCommand::Clipboard(selection))
            .await
            .map_err(|_| SupervisorError::Stopped)
    }

    fn ensure_running(&self) -> Result<(), SupervisorError> {
        match self.state() {
            SessionState::Starting | SessionState::Active | SessionState::Detached => Ok(()),
            SessionState::Stopping | SessionState::Stopped | SessionState::Failed => {
                Err(SupervisorError::Stopped)
            }
        }
    }

    pub async fn shutdown(self) -> Result<(), SupervisorError> {
        set_state(&self.state, SessionState::Stopping);
        let _ = self.capture_tx.send(CaptureCommand::Stop).await;
        let _ = self.media_tx.send(MediaCommand::Stop).await;
        self.capture_task.await.map_err(SupervisorError::TaskJoin)?;
        self.media_task.await.map_err(SupervisorError::TaskJoin)?;
        set_state(&self.state, SessionState::Stopped);
        info!(session_id = %self.session_id, "persistent AetherDesk session stopped");
        Ok(())
    }
}

async fn run_capture_loop(
    compositor: &mut HeadlessCompositor,
    mut commands: mpsc::Receiver<CaptureCommand>,
    frames: watch::Sender<Option<Arc<agent_wayland::damage::CapturedFrame>>>,
    clipboard_output: broadcast::Sender<CapturedClipboard>,
    clients: Arc<Mutex<HashSet<Uuid>>>,
    mut rate: watch::Receiver<(u32, u32)>,
) -> Result<(), SupervisorError> {
    compositor.set_capture_rate(rate.borrow().1)?;
    loop {
        tokio::select! {
            changed = rate.changed() => {
                if changed.is_err() { return Err(SupervisorError::Stopped); }
                compositor.set_capture_rate(rate.borrow_and_update().1)?;
            }
            command = commands.recv() => match command {
                Some(CaptureCommand::Resize { width, height }) => compositor.resize(width, height)?,
                Some(CaptureCommand::Input(input)) => compositor.inject_input(input)?,
                Some(CaptureCommand::Clipboard(selection)) => compositor.set_clipboard(selection)?,
                Some(CaptureCommand::KeyboardLayout { layout, variant }) => compositor.set_keyboard_layout(layout, variant)?,
                Some(CaptureCommand::MonitorLayout { width, height, monitors, properties }) => compositor.set_monitor_configuration(width, height, monitors, properties)?,
                Some(CaptureCommand::Stop) | None => {
                    compositor.shutdown();
                    return Ok(());
                }
            },
            event = compositor.next_event() => {
                let event = event?;
                let attached = !clients.lock().map_err(|_| SupervisorError::Poisoned)?.is_empty();
                match event {
                    CompositorEvent::Frame(frame) => {
                        frames.send_replace(Some(frame));
                    }
                    CompositorEvent::Clipboard(clipboard) if attached => {
                        let _ = clipboard_output.send(clipboard);
                    }
                    _ => {}
                }
            }
        }
    }
}

struct OwnedCapture(Arc<agent_wayland::damage::CapturedFrame>);
impl AsRef<[u8]> for OwnedCapture {
    fn as_ref(&self) -> &[u8] {
        &self.0.pixels_bgra
    }
}

async fn run_media_loop(
    mut encoder: EncoderPipeline,
    mut commands: mpsc::Receiver<MediaCommand>,
    mut frames: watch::Receiver<Option<Arc<agent_wayland::damage::CapturedFrame>>>,
    packets: broadcast::Sender<EncodedPacket>,
    refresh: Arc<Notify>,
    mut rate: watch::Receiver<(u32, u32)>,
) -> Result<(), SupervisorError> {
    let mut paused = true;
    let mut lossless = false;
    let mut in_flight = false;
    let mut refresh_pending = false;
    let mut frame_interval = Duration::from_secs_f64(1.0 / rate.borrow().1 as f64);
    let mut next_frame = tokio::time::Instant::now();
    loop {
        let send_frame = tokio::select! {
            biased;
            command = commands.recv() => match command {
                // Gate captured frames while detached. Keeping the live pipeline
                // running avoids PAUSED/PLAYING flushing races on rapid reconnects.
                // With no frames submitted, the encoder has no work to do.
                Some(MediaCommand::Lossless(enabled)) => { lossless = enabled; false }
                Some(MediaCommand::Pause) => { paused = true; false }
                Some(MediaCommand::Resume) => { encoder.request_keyframe(); paused = false; true }
                Some(MediaCommand::Stop) | None => break,
            },
            _ = refresh.notified() => {
                encoder.request_keyframe();
                refresh_pending = true;
                !paused && !in_flight
            },
            changed = rate.changed() => {
                if changed.is_err() { break; }
                let (bitrate, fps) = *rate.borrow_and_update();
                encoder.set_bitrate(bitrate);
                encoder.set_framerate(fps)?;
                frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
                false
            },
            packet = encoder.next_packet(), if in_flight => {
                in_flight = false;
                let _ = packets.send(packet?);
                refresh_pending && !paused
            },
            _ = tokio::time::sleep_until(next_frame), if !paused && !in_flight && !lossless => {
                next_frame = tokio::time::Instant::now() + frame_interval;
                frames.has_changed().unwrap_or(false)
            }
        };
        if send_frame && !in_flight && !lossless {
            // Resume/resize and a rate update may be ready together. Apply
            // the current budget before encoding even that first refresh.
            let (bitrate, fps) = *rate.borrow_and_update();
            encoder.set_bitrate(bitrate);
            encoder.set_framerate(fps)?;
            frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
            let frame = frames.borrow_and_update().clone();
            if let Some(frame) = frame {
                encoder.resize(frame.width, frame.height)?;
                encoder.push_owned_bgra_frame(OwnedCapture(frame.clone()), frame.stride)?;
                in_flight = true;
                refresh_pending = false;
            }
        }
    }
    Ok(())
}

// React to actual blocked delivery, not low throughput on an idle desktop.
// Cooldowns prevent a keyframe recovery burst from repeatedly cutting quality.
struct MediaRate {
    bitrate: u32,
    fps: u32,
    maximum: (u32, u32),
    base_bitrate: u32,
    adjusted: Instant,
    bad_intervals: u32,
}

impl MediaRate {
    fn new(bitrate: u32, fps: u32) -> Self {
        Self {
            bitrate,
            fps,
            maximum: (bitrate, fps),
            base_bitrate: bitrate,
            adjusted: Instant::now() - Duration::from_secs(3),
            bad_intervals: 0,
        }
    }

    fn reset(&mut self) {
        (self.bitrate, self.fps) = self.maximum;
        self.adjusted = Instant::now() - Duration::from_secs(3);
        self.bad_intervals = 0;
    }

    fn resize(&mut self, width: u32, height: u32) -> bool {
        // The configured budget covers up to 1080p. Scale it for physical
        // pixels on high-DPI/multiple monitors, with a bounded upper limit.
        let pixels = u64::from(width) * u64::from(height);
        let budget = (u64::from(self.base_bitrate).saturating_mul(pixels.max(1920 * 1080))
            / (1920 * 1080))
            .min(u64::from(self.base_bitrate.max(64_000))) as u32;
        if budget == self.maximum.0 {
            return false;
        }
        self.bitrate = ((u64::from(self.bitrate) * u64::from(budget) / u64::from(self.maximum.0))
            as u32)
            .clamp(128, budget);
        self.maximum.0 = budget;
        true
    }

    fn observe(&mut self, elapsed: Duration, lagged: bool, now: Instant) -> bool {
        let congested = lagged || elapsed > Duration::from_millis(100);
        if congested {
            self.bad_intervals = self.bad_intervals.saturating_add(1);
        } else {
            self.bad_intervals = 0;
        }
        // A single bad interval is often our own frame drop or a transient
        // burst; only sustained congestion may cut fps and bitrate, so quality
        // can never spiral downward from isolated events.
        if congested && self.bad_intervals < 2 {
            return false;
        }
        let cooldown = if congested {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(8)
        };
        if now.duration_since(self.adjusted) < cooldown {
            return false;
        }
        let previous = (self.bitrate, self.fps);
        if congested {
            self.fps = (self.fps * 70 / 100).max(self.maximum.1.min(5));
            // Preserve at least half the normal bits per frame. A fixed
            // 500 kb/s floor is unusable for a large multi-monitor desktop.
            let quality_floor = self.maximum.0 * self.fps / self.maximum.1 / 2;
            self.bitrate = (self.bitrate * 65 / 100).max(quality_floor.max(128));
        } else {
            self.bitrate = (self.bitrate + self.maximum.0 / 10).min(self.maximum.0);
            self.fps = (self.fps + 2).min(self.maximum.1);
        }
        self.adjusted = now;
        previous != (self.bitrate, self.fps)
    }
}

fn set_state(state: &RwLock<SessionState>, value: SessionState) {
    if let Ok(mut state) = state.write() {
        *state = value;
    }
}

fn set_state_if_running(state: &RwLock<SessionState>, value: SessionState) {
    if let Ok(mut state) = state.write() {
        if !matches!(
            *state,
            SessionState::Stopping | SessionState::Stopped | SessionState::Failed
        ) {
            *state = value;
        }
    }
}

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("the persistent agent has stopped")]
    Stopped,
    #[error("input is rejected while no ARPP client is attached")]
    NotAttached,
    #[error("an agent synchronization primitive was poisoned")]
    Poisoned,
    #[error("compositor failed: {0}")]
    Compositor(#[from] CompositorError),
    #[error("input engine failed: {0}")]
    Input(#[from] InputError),
    #[error("encoder failed: {0}")]
    Encoder(#[from] EncoderError),
    #[error("agent task failed: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn congestion_does_not_destroy_the_per_frame_quality_budget() {
        let mut rate = MediaRate::new(8000, 30);
        let start = Instant::now();
        for index in 0..30 {
            rate.observe(
                Duration::from_millis(300),
                true,
                start + Duration::from_secs(index * 3),
            );
            let floor = 8000 * rate.fps / 30 / 2;
            assert!(
                rate.bitrate >= floor,
                "quality budget collapsed: {} kb/s at {} fps (minimum {floor})",
                rate.bitrate,
                rate.fps
            );
        }
    }

    #[test]
    fn monitor_pixels_scale_quality_and_reconnect_resets_old_congestion() {
        let mut rate = MediaRate::new(8000, 30);
        assert!(rate.resize(4800, 1800));
        assert_eq!(rate.maximum, (33333, 30));
        let now = Instant::now();
        for i in 0..20 {
            rate.observe(
                Duration::from_millis(300),
                true,
                now + Duration::from_secs(i * 3),
            );
        }
        assert!(rate.bitrate >= rate.maximum.0 * rate.fps / 30 / 2);
        rate.reset();
        assert_eq!((rate.bitrate, rate.fps), (33333, 30));
        assert!(rate.resize(1280, 720));
        assert_eq!((rate.bitrate, rate.fps), (8000, 30));
        assert!(!rate.resize(1280, 720));
        assert!(rate.resize(16384, 2048));
        assert_eq!(rate.maximum.0, 64000);
    }

    #[test]
    fn resizing_keeps_the_budget_valid_at_configuration_limits() {
        for bitrate in [128, 8000, 250_000] {
            for fps in [1, 5, 30, 240] {
                let mut rate = MediaRate::new(bitrate, fps);
                let start = Instant::now();
                rate.resize(7680, 4320);
                for index in 0..30 {
                    rate.observe(
                        Duration::from_millis(300),
                        true,
                        start + Duration::from_secs(index * 3),
                    );
                }
                for (width, height) in [(1280, 720), (u32::MAX, u32::MAX), (1920, 1080)] {
                    rate.resize(width, height);
                    assert!((128..=rate.maximum.0).contains(&rate.bitrate));
                    assert!(rate.maximum.0 <= bitrate.max(64_000));
                }
            }
        }
    }

    #[test]
    fn only_sustained_congestion_reduces_rate_and_it_recovers_with_cooldown() {
        let mut rate = MediaRate::new(8000, 30);
        let now = Instant::now();
        // A single bad interval may be our own drop or a transient burst.
        assert!(!rate.observe(Duration::from_millis(300), true, now));
        assert_eq!((rate.bitrate, rate.fps), (8000, 30));
        // Sustained congestion is what cuts fps and bitrate.
        assert!(rate.observe(Duration::from_millis(300), true, now));
        assert_eq!((rate.bitrate, rate.fps), (5200, 21));
        // The cooldown absorbs further events right after an adjustment.
        assert!(!rate.observe(Duration::from_millis(300), true, now));
        // Healthy delivery recovers gradually after the longer cooldown.
        assert!(rate.observe(
            Duration::from_millis(1),
            false,
            now + Duration::from_secs(9)
        ));
        assert_eq!((rate.bitrate, rate.fps), (6000, 23));
    }

    #[test]
    fn detached_is_not_a_terminal_state() {
        assert_ne!(SessionState::Detached, SessionState::Stopped);
        assert_ne!(SessionState::Detached, SessionState::Failed);
    }

    #[test]
    fn late_state_changes_cannot_revive_failed_sessions() {
        let state = RwLock::new(SessionState::Failed);
        set_state_if_running(&state, SessionState::Active);
        assert_eq!(*state.read().unwrap(), SessionState::Failed);
    }

    #[test]
    fn client_membership_only_transitions_on_real_changes() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut clients = HashSet::new();

        let was_empty = clients.is_empty();
        assert!(clients.insert(first) && was_empty);
        let was_empty = clients.is_empty();
        assert!(!(clients.insert(first) && was_empty));
        let was_empty = clients.is_empty();
        assert!(!(clients.insert(second) && was_empty));

        let removed = clients.remove(&first);
        assert!(!(removed && clients.is_empty()));
        let removed = clients.remove(&second);
        assert!(removed && clients.is_empty());
        let removed = clients.remove(&second);
        assert!(!(removed && clients.is_empty()));
    }
}
