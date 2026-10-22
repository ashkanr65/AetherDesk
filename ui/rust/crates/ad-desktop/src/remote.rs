//! Native owner of the production ARPP session/decoder pipeline.
//! Session pump ported from ad-ffi/remote.rs; there is no C ABI or Flutter runtime.
use crate::remote_input::InputQueue;
use ad_display_decoder::{
    CacheLimits, DecoderConfig, Framebuffer, PresentationUpdate, RemotePresentation, VideoDecoder,
};
use ad_remote_session::{
    AgentArtifact, AgentRuntimeBundle, ClipboardBackend, ClipboardBridge, ClipboardContent,
    ClipboardError, ClipboardLimits, DisconnectMode, RemoteSessionLifecycle, SshAgentHost,
};
use ad_stream_client::{ArppSession, StreamDemuxer};
use ad_stream_protocol::{
    ArppMessage, ControlMessage, InputMessage, MetricsMessage, SessionParameters, VideoCodec,
};
use async_trait::async_trait;
use eframe::egui;
use parking_lot::Mutex;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct Status {
    pub state: &'static str,
    pub error: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub clipboard: Option<String>,
}
#[derive(Clone)]
pub struct ImageDamage {
    pub base_sequence: u64,
    pub regions: Vec<ad_stream_protocol::Rectangle>,
}
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub pixels: Arc<Vec<u8>>,
    pub damage: Option<ImageDamage>,
}
struct FrameSlot {
    cursor: Mutex<Option<Arc<ad_display_decoder::CursorState>>>,
    image: Mutex<Option<Image>>,
    prepared: Arc<Mutex<Option<crate::remote_frames::PreparedFrame>>>,
    ctx: egui::Context,
}
struct BackgroundTasks(Vec<tokio::task::AbortHandle>);
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
pub enum Request {
    Input {
        event: InputMessage,
    },
    Clipboard {
        text: String,
    },
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
}
enum Command {
    Request(Request),
    Stop(DisconnectMode, oneshot::Sender<Result<(), String>>),
}
struct ClipboardSink(Arc<Mutex<Status>>);
#[async_trait]
impl ClipboardBackend for ClipboardSink {
    async fn set_content(&self, content: ClipboardContent) -> Result<(), ClipboardError> {
        if content.mime_type.starts_with("text/plain") {
            self.0.lock().clipboard =
                Some(String::from_utf8(content.data).map_err(|_| ClipboardError::InvalidText)?);
        }
        Ok(())
    }
}
pub struct RemoteClient {
    window_target: Option<crate::remote_applications::Target>,
    commands: mpsc::Sender<Command>,
    input: Arc<InputQueue>,
    status: Arc<Mutex<Status>>,
    latest: Arc<FrameSlot>,
    task: Mutex<Option<JoinHandle<()>>>,
    newly_created: bool,
}
impl RemoteClient {
    pub async fn start(
        ssh: Arc<ad_ssh::SshSession>,
        desktop_environment: String,
        session_id: Uuid,
        ctx: egui::Context,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Arc<Self>, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let base = executable.parent().ok_or("Application directory missing")?;
        // Configure only the bundled decoder. Never depend on an end-user SDK.
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            std::env::set_var("GST_PLUGIN_PATH", base.join("gstreamer-1.0"));
            std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", base.join("gstreamer-1.0"));
            std::env::set_var("GST_PLUGIN_SCANNER", base.join("gst-plugin-scanner.exe"));
        });
        let bundle = base.join("agent/linux-x86_64");
        let agent_version = tokio::fs::read_to_string(bundle.join("version.txt"))
            .await
            .map_err(|_| "Bundled Linux agent version missing. Extract the complete package.")?;
        let artifact =
            AgentArtifact::inspect(agent_version.trim().to_owned(), bundle.join("ad-agent"))
                .await
                .map_err(|e| e.to_string())?;
        let runtime = AgentRuntimeBundle::inspect(&bundle)
            .await
            .map_err(|e| e.to_string())?;
        let decoder = VideoDecoder::new(DecoderConfig::default()).map_err(|e| e.to_string())?;
        let (width, height) = (1280, 720);
        let presentation = RemotePresentation::new(width, height, CacheLimits::default())
            .map_err(|e| e.to_string())?;
        let framebuffer = Framebuffer::new(width, height).map_err(|e| e.to_string())?;
        let parameters = SessionParameters {
            session_id,
            desktop_environment,
            width,
            height,
            scale_milli: 1000,
            monitors: 1,
            video_codec: VideoCodec::H264,
            audio_codec: None,
            target_fps: 30,
            target_bitrate_kbps: 8000,
        };
        let mut lifecycle =
            RemoteSessionLifecycle::new(Arc::new(SshAgentHost::new(ssh.clone())), session_id);
        lifecycle.enable_lossless_tiles(std::env::var_os("AETHERDESK_DESKTOP_VIDEO").is_none());
        lifecycle.enable_cursor_shapes(true);
        let host = ssh.clone();
        let started = std::time::Instant::now();
        let session = match lifecycle
            .connect_saved_desktop_cancellable(&artifact, Some(&runtime), &parameters, cancel)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                tracing::warn!(%session_id, elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %error, "remote desktop startup failed");
                if !cancel.is_cancelled() {
                    log_agent_failure(&host, session_id, "startup").await;
                }
                return Err(error.to_string());
            }
        };
        let status = Arc::new(Mutex::new(Status {
            state: "connected",
            error: None,
            width,
            height,
            frames: 0,
            clipboard: None,
        }));
        let latest = Arc::new(FrameSlot {
            cursor: Mutex::new(None),
            image: Mutex::new(None),
            prepared: Arc::new(Mutex::new(None)),
            ctx: ctx.clone(),
        });
        let input = Arc::new(InputQueue::default());
        let (commands, receive) = mpsc::channel(256);
        let handle = Arc::new(Self {
            window_target: Some(crate::remote_applications::Target {
                ssh,
                session: session_id,
                version: agent_version.trim().to_owned(),
                runtime: tokio::runtime::Handle::current(),
            }),
            commands,
            input: input.clone(),
            status: status.clone(),
            latest: latest.clone(),
            task: Mutex::new(None),
            newly_created: lifecycle.newly_created(),
        });
        let task = tokio::spawn(async move {
            let result = run_session(
                &mut lifecycle,
                session,
                receive,
                input,
                status.clone(),
                latest,
                decoder,
                presentation,
                framebuffer,
            )
            .await;
            if let Err(error) = result {
                tracing::warn!(%session_id, error = %error, "remote desktop stream failed; retaining server desktop");
                log_agent_failure(&host, session_id, "stream").await;
                let _ = tokio::time::timeout(
                    Duration::from_secs(3),
                    lifecycle.disconnect(DisconnectMode::Detach),
                )
                .await;
                let mut state = status.lock();
                state.state = "failed";
                state.error = Some(error);
            } else {
                status.lock().state = "closed";
            }
            ctx.request_repaint();
        });
        *handle.task.lock() = Some(task);
        Ok(handle)
    }
    #[cfg(test)]
    pub(crate) fn test_stub(ctx: egui::Context) -> Arc<Self> {
        let (commands, receiver) = mpsc::channel(1);
        drop(receiver);
        Arc::new(Self {
            window_target: None,
            commands,
            input: Arc::new(InputQueue::default()),
            status: Arc::new(Mutex::new(Status {
                state: "connected",
                error: None,
                width: 1280,
                height: 720,
                frames: 0,
                clipboard: None,
            })),
            latest: Arc::new(FrameSlot {
                cursor: Mutex::new(None),
                image: Mutex::new(None),
                prepared: Arc::new(Mutex::new(None)),
                ctx,
            }),
            task: Mutex::new(None),
            newly_created: false,
        })
    }

    pub fn peek_status(&self) -> Status {
        self.status.lock().clone()
    }
    pub fn newly_created(&self) -> bool {
        self.newly_created
    }
    pub(crate) fn window_target(&self) -> Option<crate::remote_applications::Target> {
        self.window_target.clone()
    }
    pub fn status(&self) -> Status {
        let mut status = self.status.lock();
        let result = status.clone();
        status.clipboard = None;
        result
    }
    pub(crate) fn cursor(&self) -> Option<Arc<ad_display_decoder::CursorState>> {
        self.latest.cursor.lock().clone()
    }
    pub fn frame(&self) -> Option<Image> {
        self.latest.image.lock().clone()
    }
    pub fn prepared_after(&self, sequence: u64) -> Option<crate::remote_frames::PreparedFrame> {
        self.latest
            .prepared
            .lock()
            .as_ref()
            .filter(|frame| frame.sequence != sequence)
            .cloned()
    }
    pub fn send(&self, request: Request) -> Result<(), String> {
        match &request {
            Request::Input { event } => return self.input.push(event.clone()),
            Request::KeyboardLayout { layout, variant } => {
                return self.input.control(ControlMessage::KeyboardLayout {
                    layout: layout.clone(),
                    variant: variant.clone(),
                })
            }
            Request::MonitorLayout {
                width,
                height,
                monitors,
                properties,
            } => {
                return self
                    .input
                    .control(ControlMessage::MonitorLayoutWithProperties {
                        width: *width,
                        height: *height,
                        monitors: monitors.clone(),
                        properties: properties.clone(),
                    })
            }
            Request::Clipboard { text } if text.len() > 1024 * 1024 => {
                return Err("Clipboard text exceeds 1 MiB".into())
            }
            _ => {}
        }
        self.commands
            .try_send(Command::Request(request))
            .map_err(|_| "The desktop control queue is busy or closed".into())
    }
    pub async fn stop(&self, end: bool) -> Result<(), String> {
        let (reply, receive) = oneshot::channel();
        let mode = if end {
            DisconnectMode::Terminate
        } else {
            DisconnectMode::Detach
        };
        let result = tokio::time::timeout(Duration::from_secs(12), async {
            if self
                .commands
                .send(Command::Stop(mode, reply))
                .await
                .is_err()
            {
                return if end {
                    Err(
                        "Remote desktop already closed; its termination was not confirmed"
                            .to_owned(),
                    )
                } else {
                    Ok(())
                };
            }
            receive
                .await
                .map_err(|_| "Remote shutdown was not confirmed".to_owned())?
        })
        .await
        .unwrap_or_else(|_| Err("Remote shutdown confirmation timed out".to_owned()));
        let task = self.task.lock().take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(3), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        result
    }
}
#[allow(clippy::too_many_arguments)]
async fn run_session(
    lifecycle: &mut RemoteSessionLifecycle<SshAgentHost>,
    session: Arc<ArppSession>,
    mut commands: mpsc::Receiver<Command>,
    input: Arc<InputQueue>,
    status: Arc<Mutex<Status>>,
    latest: Arc<FrameSlot>,
    decoder: VideoDecoder,
    presentation: RemotePresentation,
    framebuffer: Framebuffer,
) -> Result<(), String> {
    let (mut demuxer, mut channels) = StreamDemuxer::spawn(session.clone());
    let clipboard = ClipboardBridge::new(
        Arc::new(ClipboardSink(status.clone())),
        session.clone(),
        ClipboardLimits::default(),
    );
    let input_session = session.clone();
    let mut input_task = tokio::spawn(async move {
        loop {
            let event = input.receive().await?;
            tokio::time::timeout(Duration::from_secs(5), input_session.send(event))
                .await
                .map_err(|_| "Remote input transport stalled".to_owned())?
                .map_err(|e| e.to_string())?;
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    });
    let (frame_updates, frame_receiver) = watch::channel(None);
    let mut preparation_task = tokio::spawn(crate::remote_frames::prepare(
        frame_receiver,
        latest.prepared.clone(),
        latest.ctx.clone(),
    ));
    let mut display_task = tokio::spawn(run_display(
        channels.display,
        channels.receipts,
        status,
        latest,
        decoder,
        presentation,
        framebuffer,
        frame_updates,
    ));
    let (clipboard_tx, mut clipboard_rx) = watch::channel::<Option<String>>(None);
    let mut clipboard_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                message = channels.clipboard.recv() => {
                    clipboard.handle_remote(message.ok_or("clipboard channel closed")?).await.map_err(|e| e.to_string())?;
                }
                changed = clipboard_rx.changed() => {
                    changed.map_err(|_| "clipboard sender closed")?;
                    let text = clipboard_rx.borrow_and_update().clone();
                    if let Some(text) = text {
                        clipboard.publish_local(vec![ClipboardContent { mime_type: "text/plain;charset=utf-8".into(), data: text.into_bytes() }]).await.map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    });
    let _workers = BackgroundTasks(vec![
        input_task.abort_handle(),
        display_task.abort_handle(),
        preparation_task.abort_handle(),
        clipboard_task.abort_handle(),
    ]);
    let mut heartbeat = desktop_heartbeat();
    let mut nonce = 0_u64;
    let result = async {
        loop {
            tokio::select! {
                result = &mut input_task => return result.map_err(|e| e.to_string())?,
                result = &mut display_task => return result.map_err(|e| e.to_string())?,
                result = &mut preparation_task => {
                    result.map_err(|e| e.to_string())??;
                    return Err("Desktop frame preparation stopped".into());
                },
                result = &mut clipboard_task => return result.map_err(|e| e.to_string())?,
                command = commands.recv() => {
                    match command {
                        Some(Command::Request(Request::Input { .. } | Request::KeyboardLayout { .. } | Request::MonitorLayout { .. })) => return Err("Input configuration was routed to the control queue".into()),
                        Some(Command::Request(Request::Clipboard { text })) => {
                            clipboard_tx.send_replace(Some(text));
                        }
                        Some(Command::Stop(mode, reply)) => {
                            // Lifecycle owns the close acknowledgement; stop
                            // the competing receiver before sending the request.
                            demuxer.shutdown_and_wait().await;
                            let result = lifecycle.disconnect(mode).await.map_err(|e| e.to_string());
                            let _ = reply.send(result.clone());
                            return result;
                        }
                        None => { demuxer.shutdown_and_wait().await; lifecycle.disconnect(DisconnectMode::Detach).await.map_err(|e| e.to_string())?; return Ok(()); }
                    }
                }
                message = channels.control.recv() => {
                    if let ControlMessage::Close { detail, .. } = message.ok_or("control channel closed")? { return Err(detail); }
                }
                message = channels.metrics.recv() => {
                    if let MetricsMessage::Ping { nonce, sent_at_us } = message.ok_or("metrics channel closed")? {
                        session.send(ArppMessage::Metrics(MetricsMessage::Pong { nonce, sent_at_us, received_at_us: sent_at_us })).await.map_err(|e| e.to_string())?;
                    }
                }
                message = channels.audio.recv() => { message.ok_or("audio channel closed")?; }
                message = channels.bulk.recv() => { message.ok_or("file channel closed")?; }
                changed = channels.errors.changed() => {
                    changed.map_err(|_| "transport stopped".to_owned())?;
                    if let Some(error) = channels.errors.borrow().clone() { return Err(error); }
                }
                _ = heartbeat.tick() => {
                    nonce += 1;
                    tokio::time::timeout(Duration::from_secs(5), session.send(ArppMessage::Metrics(MetricsMessage::Ping { nonce, sent_at_us: nonce * 2_000_000 })))
                        .await.map_err(|_| "Remote heartbeat transport stalled after idle or network interruption".to_owned())?
                        .map_err(|e| e.to_string())?;
                }
            };
        }
    }.await;
    // The receiver records the transport error before dropping its senders.
    // Prefer that evidence over a racing generic 'control channel closed'.
    let result = result.map_err(|error| recorded_transport_error(&channels.errors, error));
    demuxer.shutdown_and_wait().await;
    result
}

fn desktop_heartbeat() -> tokio::time::Interval {
    desktop_heartbeat_at(tokio::time::Instant::now())
}
fn desktop_heartbeat_at(start: tokio::time::Instant) -> tokio::time::Interval {
    let mut interval = tokio::time::interval_at(start, Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval
}
fn agent_diagnostic_command(session: Uuid) -> String {
    // Fixed command and parsed UUID only; never read another account's home,
    // application files, endpoint credentials or full unbounded logs.
    format!("tail -c 8192 -- \"$HOME/.aetherdesk/sessions/{session}/agent.log\" 2>/dev/null")
}
async fn log_agent_failure(ssh: &ad_ssh::SshSession, session: Uuid, phase: &str) {
    if ssh.is_closed() {
        return;
    }
    match ssh
        .execute_command_output(&agent_diagnostic_command(session), Duration::from_secs(2))
        .await
    {
        Ok(output) if output.exit_status == 0 => {
            let text = String::from_utf8_lossy(&output.stdout);
            let text = text
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .take(8192)
                .collect::<String>();
            let diagnostic = crate::logging::redact("agent_diagnostic", &text);
            tracing::warn!(%session, %phase, %diagnostic, "remote agent failure evidence");
        }
        _ => {
            tracing::warn!(%session, %phase, "remote agent log unavailable; retain the server session agent.log for diagnosis")
        }
    }
}
fn recorded_transport_error(errors: &watch::Receiver<Option<String>>, fallback: String) -> String {
    errors.borrow().clone().unwrap_or(fallback)
}
#[cfg(test)]
mod transport_error_tests {
    use super::*;
    #[tokio::test]
    async fn waking_after_idle_does_not_burst_missed_heartbeats() {
        let mut interval =
            desktop_heartbeat_at(tokio::time::Instant::now() - Duration::from_secs(60));
        interval.tick().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), interval.tick())
                .await
                .is_err(),
            "wake-up replayed a queued heartbeat immediately"
        );
    }
    #[test]
    fn failure_diagnostics_read_only_the_current_agents_bounded_log() {
        let command = agent_diagnostic_command(Uuid::nil());
        assert!(command.starts_with("tail -c 8192 -- \"$HOME/.aetherdesk/sessions/"));
        assert!(command.contains("00000000-0000-0000-0000-000000000000/agent.log"));
        assert!(!command.contains("endpoint"));
        assert!(!command.contains("kill"));
        assert_eq!(
            crate::logging::redact("agent_diagnostic", "token=private"),
            "<redacted sensitive detail>"
        );
    }
    #[test]
    fn channel_close_races_preserve_the_recorded_transport_failure() {
        let (send, errors) = watch::channel(None);
        assert_eq!(
            recorded_transport_error(&errors, "control channel closed".into()),
            "control channel closed"
        );
        send.send_replace(Some("ARPP media channel: unexpected end of file".into()));
        assert_eq!(
            recorded_transport_error(&errors, "control channel closed".into()),
            "ARPP media channel: unexpected end of file"
        );
    }
}
async fn run_display(
    mut display: mpsc::Receiver<ad_stream_protocol::DisplayMessage>,
    receipts: watch::Sender<u64>,
    status: Arc<Mutex<Status>>,
    latest: Arc<FrameSlot>,
    mut decoder: VideoDecoder,
    mut presentation: RemotePresentation,
    mut framebuffer: Framebuffer,
    frame_updates: watch::Sender<Option<Image>>,
) -> Result<(), String> {
    enum Work {
        Legacy(PresentationUpdate),
        Lossless(ad_display_decoder::LosslessUpdate),
    }
    let mut lossless = ad_display_decoder::LosslessAssembler::default();
    let mut reported_at = std::time::Instant::now();
    let (mut commits, mut apply_us) = (0_u64, 0_u128);
    loop {
        let update = tokio::select! {
            message = display.recv() => {
                let message = message.ok_or("display channel closed")?;
                if let ad_stream_protocol::DisplayMessage::CursorUpdate { width, height, rgba, .. } = &message {
                    if *width > 256 || *height > 256 || rgba.len() > 256 * 256 * 4 { return Err("Host cursor exceeds the shape limit".into()); }
                    // Validate normally but keep cursor changes out of the framebuffer,
                    // video decoder, desktop conversions and full texture uploads.
                    if let PresentationUpdate::Cursor(cursor) = presentation.handle(message).map_err(|e| e.to_string())? {
                        *latest.cursor.lock() = Some(Arc::new(cursor));
                        latest.ctx.request_repaint();
                    }
                    continue;
                }
                if ad_display_decoder::LosslessAssembler::handles(&message) {
                    lossless.push(message).map_err(str::to_owned)?.map(Work::Lossless)
                } else { match presentation.handle(message).map_err(|e| e.to_string())? {
                    PresentationUpdate::Decode { message, damage } => {
                        if let ad_stream_protocol::DisplayMessage::EncodedFrame { width, height, .. } = &message {
                            decoder.prepare_dimensions(*width, *height).map_err(|e| e.to_string())?;
                        }
                        decoder.submit(&message, damage).await.map_err(|e| e.to_string())?;
                        None
                    }
                    update => Some(Work::Legacy(update)),
                } }
            }
            frame = decoder.next_frame() => Some(Work::Legacy(presentation.accept_decoded(frame.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?)),
        };
        if let Some(update) = update {
            let started = std::time::Instant::now();
            let (decoded_sequence, damage, changed) = match update {
                Work::Lossless(update) => {
                    let sequence = update.frame_id;
                    let damage = (update.base_frame_id != 0).then(|| ImageDamage {
                        base_sequence: status.lock().frames,
                        regions: update.patches.iter().map(|p| p.region).collect(),
                    });
                    framebuffer
                        .apply_lossless(update)
                        .map_err(|e| e.to_string())?;
                    (Some(sequence), damage, true)
                }
                Work::Legacy(update) => {
                    let decoded_sequence = match &update {
                        PresentationUpdate::VideoLayer(frame) => Some(frame.frame_id),
                        _ => None,
                    };
                    let changed = framebuffer.apply_owned(update).map_err(|e| e.to_string())?;
                    (decoded_sequence, None, changed)
                }
            };
            if changed {
                let (width, height, pixels) = framebuffer.snapshot();
                let sequence = {
                    let mut state = status.lock();
                    state.width = width;
                    state.height = height;
                    state.frames += 1;
                    state.frames
                };
                let image = Image {
                    width,
                    height,
                    sequence,
                    pixels,
                    damage,
                };
                *latest.image.lock() = Some(image.clone());
                frame_updates.send_replace(Some(image));
                if let Some(sequence) = decoded_sequence {
                    receipts.send_replace(sequence);
                }
                commits += 1;
                apply_us += started.elapsed().as_micros();
                if reported_at.elapsed() >= Duration::from_secs(5) {
                    tracing::info!(
                        interval_ms = reported_at.elapsed().as_millis() as u64,
                        frames = commits,
                        apply_us = apply_us as u64,
                        width,
                        height,
                        "remote client presentation performance"
                    );
                    reported_at = std::time::Instant::now();
                    (commits, apply_us) = (0, 0);
                }
            }
        }
    }
}
