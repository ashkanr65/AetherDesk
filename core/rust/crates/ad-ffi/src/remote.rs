//! Remote-session C ABI. Network and decoder work runs on the core runtime.

use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
use lazy_static::lazy_static;
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::bridge::{
    clear_error, into_c_string, required_string, session_by_id, set_error, RUNTIME,
};

fn configure_runtime() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if let Ok(executable) = std::env::current_exe() {
            if let Some(base) = executable.parent() {
                let plugins = base.join("gstreamer-1.0");
                if plugins.is_dir() {
                    std::env::set_var("GST_PLUGIN_PATH", &plugins);
                    std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugins);
                    std::env::set_var("GST_PLUGIN_SCANNER", base.join("gst-plugin-scanner.exe"));
                }
            }
        }
    });
}

pub(crate) fn available() -> bool {
    configure_runtime();
    cfg!(target_os = "windows")
        && std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|base| base.join("agent/linux-x86_64")))
            .is_some_and(|path| {
                path.join("ad-agent").is_file() && path.join("runtime-manifest.json").is_file()
            })
        && VideoDecoder::discover(VideoCodec::H264).is_ok()
}

lazy_static! {
    static ref SESSIONS: Mutex<HashMap<String, Arc<RemoteHandle>>> = Mutex::new(HashMap::new());
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteConfig {
    #[serde(default)]
    desktop_session_id: Option<Uuid>,
    agent_path: PathBuf,
    runtime_directory: PathBuf,
    agent_version: String,
    desktop_environment: String,
    width: u32,
    height: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    state: &'static str,
    error: Option<String>,
    width: u32,
    height: u32,
    frames: u64,
    clipboard: Option<String>,
}

struct Image {
    width: u32,
    height: u32,
    sequence: u64,
    pixels: Arc<Vec<u8>>,
}

struct RemoteHandle {
    commands: mpsc::Sender<Command>,
    input: Arc<crate::input::InputQueue>,
    status: Arc<Mutex<Status>>,
    latest: Arc<FrameSlot>,
    task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct FrameSlot {
    image: Mutex<Option<Image>>,
    ready: Condvar,
}

struct BackgroundTasks(Vec<tokio::task::AbortHandle>);
impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Input {
        event: InputMessage,
    },
    Resize {
        width: u32,
        height: u32,
    },
    Clipboard {
        text: String,
    },
    KeyboardLayout {
        layout: String,
        variant: String,
    },
    MonitorLayout {
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
            let text = String::from_utf8(content.data).map_err(|_| ClipboardError::InvalidText)?;
            self.0.lock().clipboard = Some(text);
        }
        Ok(())
    }
}

fn get_handle(pointer: *const c_char) -> Result<Arc<RemoteHandle>, String> {
    let id = required_string(pointer, "remote session id")?;
    SESSIONS
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| "remote session was closed".into())
}

/// Start an embedded desktop over an existing, authenticated SSH session.
/// Returns an owned session-id string, released with `ad_free_string`.
#[no_mangle]
pub extern "C" fn ad_remote_start(ssh_id: *const c_char, config: *const c_char) -> *mut c_char {
    clear_error();
    configure_runtime();
    let result = (|| -> Result<String, String> {
        let (_, ssh) = session_by_id(ssh_id)?;
        let config = required_string(config, "remote configuration")?;
        let config: RemoteConfig =
            serde_json::from_str(&config).map_err(|error| error.to_string())?;
        // Validate allocations before opening a remote session.
        let framebuffer =
            Framebuffer::new(config.width, config.height).map_err(|error| error.to_string())?;
        RUNTIME.block_on(async move {
            let decoder =
                VideoDecoder::new(DecoderConfig::default()).map_err(|error| error.to_string())?;
            let presentation =
                RemotePresentation::new(config.width, config.height, CacheLimits::default())
                    .map_err(|error| error.to_string())?;
            let artifact = AgentArtifact::inspect(config.agent_version, config.agent_path)
                .await
                .map_err(|error| error.to_string())?;
            let runtime = AgentRuntimeBundle::inspect(config.runtime_directory)
                .await
                .map_err(|error| error.to_string())?;
            let session_id = config.desktop_session_id.unwrap_or_else(Uuid::new_v4);
            let parameters = SessionParameters {
                session_id,
                desktop_environment: config.desktop_environment,
                width: config.width,
                height: config.height,
                scale_milli: 1000,
                monitors: 1,
                video_codec: VideoCodec::H264,
                audio_codec: None,
                target_fps: 30,
                target_bitrate_kbps: 8000,
            };
            let mut lifecycle =
                RemoteSessionLifecycle::new(Arc::new(SshAgentHost::new(ssh)), session_id);
            let session = lifecycle
                .connect_saved_desktop(&artifact, Some(&runtime), &parameters)
                .await
                .map_err(|error| error.to_string())?;
            let status = Arc::new(Mutex::new(Status {
                state: "connected",
                error: None,
                width: config.width,
                height: config.height,
                frames: 0,
                clipboard: None,
            }));
            let latest = Arc::new(FrameSlot::default());
            let input = Arc::new(crate::input::InputQueue::default());
            let (commands, receive) = mpsc::channel(256);
            let handle = Arc::new(RemoteHandle {
                commands,
                input: input.clone(),
                status: status.clone(),
                latest: latest.clone(),
                task: Mutex::new(None),
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
            });
            *handle.task.lock() = Some(task);
            // The local view handle is independent from the persistent host desktop.
            let id = Uuid::new_v4().to_string();
            SESSIONS.lock().insert(id.clone(), handle);
            Ok(id)
        })
    })();
    match result {
        Ok(id) => into_c_string(id),
        Err(error) => {
            set_error(error);
            std::ptr::null_mut()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_session(
    lifecycle: &mut RemoteSessionLifecycle<SshAgentHost>,
    session: Arc<ArppSession>,
    mut commands: mpsc::Receiver<Command>,
    input: Arc<crate::input::InputQueue>,
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
            tokio::time::timeout(
                Duration::from_secs(5),
                input_session.send(ArppMessage::Input(event)),
            )
            .await
            .map_err(|_| "Remote input transport stalled".to_owned())?
            .map_err(|e| e.to_string())?;
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    });
    let mut display_task = tokio::spawn(run_display(
        channels.display,
        channels.receipts,
        status,
        latest,
        decoder,
        presentation,
        framebuffer,
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
        clipboard_task.abort_handle(),
    ]);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(2));
    let mut nonce = 0_u64;
    let result = async {
        loop {
            tokio::select! {
                result = &mut input_task => return result.map_err(|e| e.to_string())?,
                result = &mut display_task => return result.map_err(|e| e.to_string())?,
                result = &mut clipboard_task => return result.map_err(|e| e.to_string())?,
                command = commands.recv() => {
                    match command {
                        Some(Command::Request(Request::Input { .. })) => return Err("Input was routed to the control queue".into()),
                        Some(Command::Request(Request::Resize { width, height })) => {
                            session.send(ArppMessage::Control(ControlMessage::Resize { width, height, scale_milli: 1000 })).await.map_err(|e| e.to_string())?;
                        }
                        Some(Command::Request(Request::KeyboardLayout { layout, variant })) => {
                            session.send(ArppMessage::Control(ControlMessage::KeyboardLayout { layout, variant })).await.map_err(|e| e.to_string())?;
                        }
                        Some(Command::Request(Request::MonitorLayout { width, height, monitors })) => {
                            session.send(ArppMessage::Control(ControlMessage::MonitorLayout { width, height, monitors })).await.map_err(|e| e.to_string())?;
                        }
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
                        None => { lifecycle.disconnect(DisconnectMode::Detach).await.map_err(|e| e.to_string())?; return Ok(()); }
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
                    session.send(ArppMessage::Metrics(MetricsMessage::Ping { nonce, sent_at_us: nonce * 2_000_000 })).await.map_err(|e| e.to_string())?;
                }
            };
        }
    }.await;
    demuxer.shutdown();
    result
}

async fn run_display(
    mut display: mpsc::Receiver<ad_stream_protocol::DisplayMessage>,
    receipts: watch::Sender<u64>,
    status: Arc<Mutex<Status>>,
    latest: Arc<FrameSlot>,
    mut decoder: VideoDecoder,
    mut presentation: RemotePresentation,
    mut framebuffer: Framebuffer,
) -> Result<(), String> {
    loop {
        let update = tokio::select! {
            message = display.recv() => {
                match presentation.handle(message.ok_or("display channel closed")?).map_err(|e| e.to_string())? {
                    PresentationUpdate::Decode { message, damage } => {
                        if let ad_stream_protocol::DisplayMessage::EncodedFrame { width, height, .. } = &message {
                            decoder.prepare_dimensions(*width, *height).map_err(|e| e.to_string())?;
                        }
                        decoder.submit(&message, damage).await.map_err(|e| e.to_string())?;
                        None
                    }
                    update => Some(update),
                }
            }
            frame = decoder.next_frame() => Some(presentation.accept_decoded(frame.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?),
        };
        if let Some(update) = update {
            let decoded_sequence = match &update {
                PresentationUpdate::VideoLayer(frame) => Some(frame.frame_id),
                _ => None,
            };
            if framebuffer.apply_owned(update).map_err(|e| e.to_string())? {
                let (width, height, pixels) = framebuffer.snapshot();
                let sequence = {
                    let mut state = status.lock();
                    state.width = width;
                    state.height = height;
                    state.frames += 1;
                    state.frames
                };
                *latest.image.lock() = Some(Image {
                    width,
                    height,
                    sequence,
                    pixels,
                });
                latest.ready.notify_one();
                if let Some(sequence) = decoded_sequence {
                    receipts.send_replace(sequence);
                }
            }
        }
    }
}

/// Queue input, resize, or clipboard commands. Returns -1 on invalid data or overload.
#[no_mangle]
pub extern "C" fn ad_remote_command(id: *const c_char, json: *const c_char) -> c_int {
    clear_error();
    let result = (|| -> Result<(), String> {
        let handle = get_handle(id)?;
        let json = required_string(json, "remote command")?;
        if json.len() > 10 * 1024 * 1024 {
            return Err("remote command is too large".into());
        }
        let request: Request = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        if let Request::Input { event } = request {
            return handle.input.push(event);
        }
        match &request {
            Request::Resize { width, height }
                if !ad_stream_protocol::valid_desktop_size(*width, *height) =>
            {
                return Err("Desktop dimensions exceed the supported size".into())
            }
            Request::MonitorLayout {
                width,
                height,
                monitors,
            } if !ad_stream_protocol::valid_monitor_layout(*width, *height, monitors) => {
                return Err("Invalid monitor arrangement".into())
            }
            Request::KeyboardLayout { layout, variant }
                if !ad_stream_protocol::valid_keyboard_layout(layout, variant) =>
            {
                return Err("Invalid keyboard layout".into())
            }
            _ => {}
        }
        handle
            .commands
            .try_send(Command::Request(request))
            .map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_error(error);
            -1
        }
    }
}

/// Return session state and the latest received text clipboard update as JSON.
#[no_mangle]
pub extern "C" fn ad_remote_status(id: *const c_char) -> *mut c_char {
    clear_error();
    match get_handle(id) {
        Ok(handle) => {
            let mut state = handle.status.lock();
            let json = serde_json::to_string(&*state);
            state.clipboard = None;
            match json {
                Ok(json) => into_c_string(json),
                Err(error) => {
                    set_error(error.to_string());
                    std::ptr::null_mut()
                }
            }
        }
        Err(error) => {
            set_error(error);
            std::ptr::null_mut()
        }
    }
}

/// Owned BGRA frame; valid until passed once to `ad_remote_release_frame`.
#[repr(C)]
pub struct RemoteFrame {
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub data: *mut u8,
    pub length: usize,
    pixels: Arc<Vec<u8>>,
}

/// Take the latest complete frame. Null means no new frame or a closed session.
#[no_mangle]
pub extern "C" fn ad_remote_acquire_frame(id: *const c_char) -> *mut RemoteFrame {
    let Ok(handle) = get_handle(id) else {
        return std::ptr::null_mut();
    };
    let Some(image) = handle.latest.image.lock().take() else {
        return std::ptr::null_mut();
    };
    own_frame(image)
}

/// Wait on the texture worker, never on Flutter's UI thread.
#[no_mangle]
pub extern "C" fn ad_remote_wait_frame(id: *const c_char, timeout_ms: u32) -> *mut RemoteFrame {
    let Ok(handle) = get_handle(id) else {
        return std::ptr::null_mut();
    };
    let mut image = handle.latest.image.lock();
    if image.is_none() {
        handle.latest.ready.wait_for(
            &mut image,
            Duration::from_millis(timeout_ms.min(100) as u64),
        );
    }
    image.take().map(own_frame).unwrap_or(std::ptr::null_mut())
}

fn own_frame(image: Image) -> *mut RemoteFrame {
    let length = image.pixels.len();
    let data = image.pixels.as_ptr() as *mut u8;
    Box::into_raw(Box::new(RemoteFrame {
        width: image.width,
        height: image.height,
        sequence: image.sequence,
        data,
        length,
        pixels: image.pixels,
    }))
}

/// Release a frame returned by `ad_remote_acquire_frame`. Null is accepted.
///
/// # Safety
/// A non-null pointer must be an unreleased frame returned by this library.
#[no_mangle]
pub unsafe extern "C" fn ad_remote_release_frame(frame: *mut RemoteFrame) {
    if !frame.is_null() {
        drop(Box::from_raw(frame));
    }
}

/// Detach from the desktop and release its local worker. Host applications keep running.
#[no_mangle]
pub extern "C" fn ad_remote_close(id: *const c_char) -> c_int {
    close_remote(id, DisconnectMode::Detach)
}

/// Explicitly end the host desktop and its applications.
#[no_mangle]
pub extern "C" fn ad_remote_end(id: *const c_char) -> c_int {
    close_remote(id, DisconnectMode::Terminate)
}

/// Detach every remaining remote session during application exit. Host
/// desktops keep running so the next connection resumes the same session.
#[no_mangle]
pub extern "C" fn ad_remote_close_all() -> c_int {
    clear_error();
    let handles: Vec<(String, Arc<RemoteHandle>)> = SESSIONS.lock().drain().collect();
    let mut failures = 0;
    for (id, handle) in handles {
        let task = handle.task.lock().take();
        let result = RUNTIME.block_on(async {
            let (reply, done) = oneshot::channel();
            tokio::time::timeout(Duration::from_secs(5), async {
                handle
                    .commands
                    .send(Command::Stop(DisconnectMode::Detach, reply))
                    .await
                    .map_err(|_| "remote worker already stopped".to_owned())?;
                done.await
                    .map_err(|_| "remote worker stopped during close".to_owned())?
            })
            .await
            .unwrap_or_else(|_| Err("remote close timed out".into()))
        });
        if let Some(task) = task {
            task.abort();
        }
        if result.is_err() {
            tracing::warn!("remote session {id} did not detach cleanly");
            failures += 1;
        }
    }
    failures
}

fn close_remote(id: *const c_char, mode: DisconnectMode) -> c_int {
    clear_error();
    let id = match required_string(id, "remote session id") {
        Ok(id) => id,
        Err(error) => {
            set_error(error);
            return -1;
        }
    };
    let Some(handle) = SESSIONS.lock().remove(&id) else {
        if mode == DisconnectMode::Terminate {
            set_error("The remote connection is closed; reconnect before ending the host session");
            return -1;
        }
        return 0;
    };
    let task = handle.task.lock().take();
    let result = RUNTIME.block_on(async {
        let (reply, done) = oneshot::channel();
        match tokio::time::timeout(Duration::from_secs(15), async {
            handle
                .commands
                .send(Command::Stop(mode, reply))
                .await
                .map_err(|_| "remote worker already stopped".to_owned())?;
            done.await
                .map_err(|_| "remote worker stopped during close".to_owned())?
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err("remote close timed out".into()),
        }
    });
    if let Some(task) = task {
        task.abort();
    }
    if let Err(error) = result {
        if mode == DisconnectMode::Terminate || handle.status.lock().state == "connected" {
            set_error(error);
            return -1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn ending_a_missing_connection_does_not_claim_to_stop_the_host() {
        let id = CString::new(Uuid::new_v4().to_string()).unwrap();
        assert_eq!(ad_remote_end(id.as_ptr()), -1);
        assert_eq!(ad_remote_close(id.as_ptr()), 0);
    }

    #[test]
    fn acquired_frame_outlives_session_and_is_consumed_once() {
        let id = CString::new(Uuid::new_v4().to_string()).unwrap();
        let (commands, _receive) = mpsc::channel(1);
        let handle = Arc::new(RemoteHandle {
            commands,
            input: Arc::new(crate::input::InputQueue::default()),
            status: Arc::new(Mutex::new(Status {
                state: "connected",
                error: None,
                width: 1,
                height: 1,
                frames: 7,
                clipboard: None,
            })),
            latest: Arc::new(FrameSlot {
                image: Mutex::new(Some(Image {
                    width: 1,
                    height: 1,
                    sequence: 7,
                    pixels: Arc::new(vec![1, 2, 3, 255]),
                })),
                ready: Condvar::new(),
            }),
            task: Mutex::new(None),
        });
        SESSIONS.lock().insert(id.to_str().unwrap().into(), handle);
        let frame = ad_remote_acquire_frame(id.as_ptr());
        assert!(!frame.is_null());
        assert!(ad_remote_acquire_frame(id.as_ptr()).is_null());
        SESSIONS.lock().remove(id.to_str().unwrap());
        unsafe {
            assert_eq!(
                ((*frame).width, (*frame).height, (*frame).sequence),
                (1, 1, 7)
            );
            assert_eq!(
                std::slice::from_raw_parts((*frame).data, (*frame).length),
                &[1, 2, 3, 255]
            );
            ad_remote_release_frame(frame);
            ad_remote_release_frame(std::ptr::null_mut());
        }
    }

    #[test]
    fn closing_a_view_detaches_and_only_explicit_end_terminates() {
        for terminate in [false, true] {
            let id = CString::new(Uuid::new_v4().to_string()).unwrap();
            let (commands, mut receive) = mpsc::channel(1);
            let (observed, result) = std::sync::mpsc::channel();
            let task = RUNTIME.spawn(async move {
                if let Some(Command::Stop(mode, reply)) = receive.recv().await {
                    observed.send(mode).unwrap();
                    let _ = reply.send(Ok(()));
                }
            });
            SESSIONS.lock().insert(
                id.to_str().unwrap().into(),
                Arc::new(RemoteHandle {
                    commands,
                    input: Arc::new(crate::input::InputQueue::default()),
                    status: Arc::new(Mutex::new(Status {
                        state: "connected",
                        error: None,
                        width: 640,
                        height: 400,
                        frames: 0,
                        clipboard: None,
                    })),
                    latest: Arc::new(FrameSlot::default()),
                    task: Mutex::new(Some(task)),
                }),
            );
            let code = if terminate {
                ad_remote_end(id.as_ptr())
            } else {
                ad_remote_close(id.as_ptr())
            };
            assert_eq!(code, 0);
            assert_eq!(
                result.recv_timeout(Duration::from_secs(1)).unwrap(),
                if terminate {
                    DisconnectMode::Terminate
                } else {
                    DisconnectMode::Detach
                }
            );
            assert!(!SESSIONS.lock().contains_key(id.to_str().unwrap()));
        }
    }

    #[test]
    fn input_command_matches_flutter_wire_shape() {
        let request: Request = serde_json::from_str(r#"{"type":"input","event":{"Keyboard":{"scan_code":30,"pressed":true,"modifiers":0,"repeat":false}}}"#).unwrap();
        assert!(matches!(
            request,
            Request::Input {
                event: InputMessage::Keyboard {
                    scan_code: 30,
                    pressed: true,
                    ..
                }
            }
        ));
        assert!(serde_json::from_str::<Request>(
            r#"{"type":"resize","width":640,"height":480,"unknown":true}"#
        )
        .is_err());
    }
}
