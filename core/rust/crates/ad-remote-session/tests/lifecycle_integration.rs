use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ad_remote_session::{
    AgentArtifact, AgentBootstrapError, AgentConnection, AgentDeployment, AgentEndpoint,
    DisconnectMode, LifecycleError, RemoteAgentHost, RemoteCommandResult, RemoteSessionLifecycle,
};
use ad_stream_client::{ResumeSnapshot, TransportAdapter, TransportError, TransportKind};
use ad_stream_protocol::{
    ArppFrame, ArppMessage, AudioCodec, Capabilities, CaptureBackend, ControlMessage,
    SessionParameters, VideoCodec, PROTOCOL_VERSION,
};
use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use uuid::Uuid;

#[derive(Default)]
struct AgentState {
    uploaded: bool,
    installed: bool,
    retained: bool,
    attached: bool,
    launches: usize,
    starts: usize,
    resumes: usize,
    ending: bool,
    end_acknowledged: bool,
    reject_end: bool,
    legacy_cleanup: bool,
    cleanup_probes_remaining: usize,
    end_delay: Duration,
    close_delay: Duration,
    cancel_deploy: Option<tokio_util::sync::CancellationToken>,
    cancel_launch: Option<tokio_util::sync::CancellationToken>,
    cancel_hello: Option<tokio_util::sync::CancellationToken>,
    cancel_resume: Option<tokio_util::sync::CancellationToken>,
    fail_open_once: bool,
}

#[derive(Default)]
struct MockConnection {
    state: Arc<Mutex<AgentState>>,
}

#[async_trait]
impl RemoteAgentHost for MockConnection {
    async fn command(
        &self,
        command: &str,
        _timeout: Duration,
    ) -> Result<RemoteCommandResult, AgentBootstrapError> {
        if let Some(cancel) = &self.state.lock().unwrap().cancel_deploy {
            cancel.cancel();
        }
        if command.contains("chmod 700") {
            self.state.lock().unwrap().installed = true;
        }
        Ok(RemoteCommandResult {
            stdout: if command.ends_with(" --version") {
                b"ad-agent 0.1.0\n".to_vec()
            } else {
                Vec::new()
            },
            stderr: Vec::new(),
            exit_status: 0,
        })
    }

    async fn upload_resumable(
        &self,
        _local_path: &Path,
        remote_path: &str,
    ) -> Result<(), AgentBootstrapError> {
        assert!(remote_path.ends_with(".part"));
        self.state.lock().unwrap().uploaded = true;
        Ok(())
    }
}

#[async_trait]
impl AgentConnection for MockConnection {
    async fn session_ended(&self, _session_id: Uuid) -> Result<bool, LifecycleError> {
        let mut state = self.state.lock().unwrap();
        if state.cleanup_probes_remaining > 0 {
            state.cleanup_probes_remaining -= 1;
            return Ok(false);
        }
        Ok(state.legacy_cleanup && !state.retained)
    }

    async fn launch_agent(
        &self,
        _deployment: &AgentDeployment,
        _session_id: Uuid,
        _parameters: Option<&SessionParameters>,
    ) -> Result<AgentEndpoint, LifecycleError> {
        let newly_created = {
            let mut state = self.state.lock().unwrap();
            assert!(state.installed);
            let fresh = !state.retained;
            state.retained = true;
            state.launches += 1;
            if let Some(cancel) = &state.cancel_launch {
                cancel.cancel();
            }
            fresh
        };
        // Exercise cancellation while launch is still in flight, rather than
        // only before or after its response.
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(AgentEndpoint {
            port: 41000,
            token: [7; 32],
            newly_created,
        })
    }

    async fn open_transport(
        &self,
        _endpoint: &AgentEndpoint,
    ) -> Result<Arc<dyn TransportAdapter>, LifecycleError> {
        {
            let mut state = self.state.lock().unwrap();
            if state.fail_open_once {
                state.fail_open_once = false;
                return Err(LifecycleError::Launch("opening transport failed".into()));
            }
        }
        Ok(Arc::new(MockTransport::new(self.state.clone())))
    }
}

struct MockTransport {
    state: Arc<Mutex<AgentState>>,
    inbound_tx: mpsc::Sender<ArppFrame>,
    inbound_rx: AsyncMutex<mpsc::Receiver<ArppFrame>>,
}

impl MockTransport {
    fn new(state: Arc<Mutex<AgentState>>) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::channel(16);
        Self {
            state,
            inbound_tx,
            inbound_rx: AsyncMutex::new(inbound_rx),
        }
    }

    async fn reply(&self, message: ArppMessage) {
        self.inbound_tx
            .send(ArppFrame::new(1, message))
            .await
            .unwrap();
    }
}

#[async_trait]
impl TransportAdapter for MockTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::SshChannels
    }

    async fn send(&self, frame: ArppFrame) -> Result<(), TransportError> {
        match frame.message {
            ArppMessage::Control(ControlMessage::ClientHello { .. }) => {
                if let Some(cancel) = &self.state.lock().unwrap().cancel_hello {
                    cancel.cancel();
                }
                self.reply(ArppMessage::Control(ControlMessage::ServerHello {
                    protocol_version: PROTOCOL_VERSION,
                    server_nonce: [9; 32],
                }))
                .await;
                self.reply(ArppMessage::Control(ControlMessage::Capabilities(
                    capabilities(),
                )))
                .await;
            }
            ArppMessage::Control(ControlMessage::StartSession(parameters)) => {
                self.state.lock().unwrap().starts += 1;
                self.state.lock().unwrap().attached = true;
                self.reply(ArppMessage::Control(ControlMessage::SessionReady {
                    session_id: parameters.session_id,
                    resumed: false,
                }))
                .await;
            }
            ArppMessage::Control(ControlMessage::ResumeSession { session_id, .. }) => {
                if let Some(cancel) = &self.state.lock().unwrap().cancel_resume {
                    cancel.cancel();
                }
                self.state.lock().unwrap().resumes += 1;
                self.state.lock().unwrap().attached = true;
                self.reply(ArppMessage::Control(ControlMessage::ResumeState {
                    session_id,
                    cache_epoch: 3,
                    width: 1280,
                    height: 720,
                    full_refresh_required: false,
                }))
                .await;
            }
            ArppMessage::Control(ControlMessage::Close { reason_code, .. }) => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.attached = false;
                    if reason_code == 0 {
                        state.retained = false;
                    }
                }
                if reason_code == 0 {
                    self.state.lock().unwrap().ending = true;
                    let delay = self.state.lock().unwrap().end_delay;
                    let inbound = self.inbound_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = inbound
                            .send(ArppFrame::new(
                                1,
                                ArppMessage::Control(ControlMessage::Close {
                                    reason_code: 0,
                                    detail: "Remote session ended".into(),
                                }),
                            ))
                            .await;
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn receive(&self) -> Result<ArppFrame, TransportError> {
        let frame = self
            .inbound_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or(TransportError::Closed)?;
        if matches!(
            &frame.message,
            ArppMessage::Control(ControlMessage::Close { reason_code: 0, .. })
        ) {
            let mut state = self.state.lock().unwrap();
            if state.reject_end {
                return Err(TransportError::Closed);
            }
            state.end_acknowledged = true;
        }
        Ok(frame)
    }

    async fn close(&self, _reason: &str) -> Result<(), TransportError> {
        let delay = self.state.lock().unwrap().close_delay;
        tokio::time::sleep(delay).await;
        let mut state = self.state.lock().unwrap();
        assert!(
            !state.ending || state.end_acknowledged || (state.legacy_cleanup && !state.retained),
            "transport closed before host termination acknowledgement"
        );
        state.attached = false;
        Ok(())
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        protocol_version: PROTOCOL_VERSION,
        video_codecs: vec![VideoCodec::H264],
        audio_codecs: vec![AudioCodec::Opus],
        capture_backends: vec![CaptureBackend::Wayland],
        maximum_width: 4096,
        maximum_height: 2160,
        maximum_monitors: 2,
        clipboard: true,
        audio: true,
        dynamic_resize: true,
        quic_media: false,
    }
}

async fn interrupted_saved_start(stage: &str, retained: bool) {
    let binary = std::env::temp_dir().join(format!("ad-agent-r1-{}", Uuid::new_v4()));
    tokio::fs::write(&binary, b"portable agent").await.unwrap();
    let artifact = AgentArtifact::inspect("0.1.0", &binary).await.unwrap();
    let connection = Arc::new(MockConnection::default());
    let cancel = tokio_util::sync::CancellationToken::new();
    {
        let mut state = connection.state.lock().unwrap();
        state.retained = retained;
        match stage {
            "before" => cancel.cancel(),
            "deploy" => state.cancel_deploy = Some(cancel.clone()),
            "launch" => state.cancel_launch = Some(cancel.clone()),
            "hello" => state.cancel_hello = Some(cancel.clone()),
            "resume" => state.cancel_resume = Some(cancel.clone()),
            "open_failure" => state.fail_open_once = true,
            _ => panic!("unknown stage"),
        }
    }
    let session_id = Uuid::new_v4();
    let parameters = SessionParameters {
        session_id,
        desktop_environment: "xfce".into(),
        width: 640,
        height: 480,
        scale_milli: 1000,
        monitors: 1,
        video_codec: VideoCodec::H264,
        audio_codec: None,
        target_fps: 30,
        target_bitrate_kbps: 4000,
    };
    let mut client = RemoteSessionLifecycle::new(connection.clone(), session_id);
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.connect_saved_desktop_cancellable(&artifact, None, &parameters, &cancel),
    )
    .await
    .unwrap();
    if stage == "open_failure" {
        assert!(matches!(result, Err(LifecycleError::Launch(_))));
    } else {
        assert!(matches!(result, Err(LifecycleError::Cancelled)));
    }
    let state = connection.state.lock().unwrap();
    assert_eq!(
        state.retained, retained,
        "cleanup must preserve only pre-existing desktops"
    );
    assert!(!state.attached, "cancel left an attached client");
    if matches!(stage, "before" | "deploy") {
        assert_eq!(state.launches, 0);
    } else if !retained {
        assert!(
            state.end_acknowledged,
            "fresh launch rollback was not acknowledged"
        );
    }
    drop(state);
    assert!(client.session().is_none());
    let _ = tokio::fs::remove_file(binary).await;
}

#[tokio::test]
async fn cancellation_before_or_during_deployment_never_launches_an_agent() {
    interrupted_saved_start("before", false).await;
    interrupted_saved_start("deploy", false).await;
}
#[tokio::test]
async fn cancellation_during_launch_rolls_back_only_fresh_desktops() {
    interrupted_saved_start("launch", false).await;
    interrupted_saved_start("launch", true).await;
}
#[tokio::test]
async fn cancelled_handshakes_release_their_transport_and_preserve_saved_desktops() {
    for stage in ["hello", "resume"] {
        interrupted_saved_start(stage, false).await;
        interrupted_saved_start(stage, true).await;
    }
}
#[tokio::test]
async fn a_failed_transport_open_reconnects_for_owned_startup_cleanup() {
    interrupted_saved_start("open_failure", false).await;
    interrupted_saved_start("open_failure", true).await;
}

#[tokio::test]
async fn deploy_connect_drop_reconnect_resume_and_terminate() {
    let binary = std::env::temp_dir().join(format!("ad-agent-{}", Uuid::new_v4()));
    tokio::fs::write(&binary, b"portable agent").await.unwrap();
    let artifact = AgentArtifact::inspect("0.1.0", &binary).await.unwrap();
    let connection = Arc::new(MockConnection::default());
    let session_id = Uuid::new_v4();
    let parameters = SessionParameters {
        session_id,
        desktop_environment: "xfce".into(),
        width: 1280,
        height: 720,
        scale_milli: 1000,
        monitors: 1,
        video_codec: VideoCodec::H264,
        audio_codec: None,
        target_fps: 60,
        target_bitrate_kbps: 8_000,
    };
    let mut lifecycle = RemoteSessionLifecycle::new(connection.clone(), session_id);

    lifecycle.connect(&artifact, &parameters).await.unwrap();
    {
        let state = connection.state.lock().unwrap();
        assert!(state.uploaded && state.installed && state.retained && state.attached);
        assert_eq!(state.launches, 1);
    }

    lifecycle.mark_transport_lost();
    {
        let mut state = connection.state.lock().unwrap();
        state.attached = false;
        assert!(state.retained, "network loss must retain the compositor");
    }

    let (_, action) = lifecycle
        .resume(ResumeSnapshot {
            last_presented_frame: 44,
            cache_epoch: 3,
            cache_digest: [5; 32],
        })
        .await
        .unwrap();
    assert!(matches!(
        action,
        ad_stream_client::ResumeAction::Accepted {
            full_refresh: false,
            ..
        }
    ));
    assert_eq!(connection.state.lock().unwrap().launches, 2);
    assert!(connection.state.lock().unwrap().attached);

    // A lost connection during termination must not be reported as success.
    connection.state.lock().unwrap().reject_end = true;
    assert!(lifecycle
        .disconnect(DisconnectMode::Terminate)
        .await
        .is_err());
    assert_ne!(
        lifecycle.state(),
        ad_remote_session::lifecycle::LifecycleState::Terminated
    );
    connection.state.lock().unwrap().reject_end = false;
    lifecycle
        .disconnect(DisconnectMode::Terminate)
        .await
        .unwrap();
    assert!(connection.state.lock().unwrap().end_acknowledged);

    // A confirmed end must survive stalled local socket cleanup rather than
    // timing out at the FFI boundary and showing a false connection failure.
    lifecycle.connect(&artifact, &parameters).await.unwrap();
    connection.state.lock().unwrap().close_delay = Duration::from_secs(60);
    tokio::time::timeout(
        Duration::from_secs(2),
        lifecycle.disconnect(DisconnectMode::Terminate),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        lifecycle.state(),
        ad_remote_session::lifecycle::LifecycleState::Terminated
    );
    connection.state.lock().unwrap().close_delay = Duration::ZERO;

    // Existing old-agent desktops can be verified by completed host cleanup,
    // but an EOF by itself (the failing attempt above) is never confirmation.
    lifecycle.connect(&artifact, &parameters).await.unwrap();
    {
        let mut state = connection.state.lock().unwrap();
        state.reject_end = true;
        state.end_acknowledged = false;
        state.legacy_cleanup = true;
        state.cleanup_probes_remaining = 2;
    }
    lifecycle
        .disconnect(DisconnectMode::Terminate)
        .await
        .unwrap();
    {
        let state = connection.state.lock().unwrap();
        assert!(!state.attached && !state.retained);
    }
    let _ = tokio::fs::remove_file(binary).await;
}

#[tokio::test]
async fn reopening_a_saved_desktop_uses_resume_after_the_client_is_recreated() {
    let binary = std::env::temp_dir().join(format!("ad-agent-{}", Uuid::new_v4()));
    tokio::fs::write(&binary, b"portable agent").await.unwrap();
    let artifact = AgentArtifact::inspect("0.1.0", &binary).await.unwrap();
    let connection = Arc::new(MockConnection::default());
    let session_id = Uuid::new_v4();
    let parameters = SessionParameters {
        session_id,
        desktop_environment: "xfce".into(),
        width: 640,
        height: 400,
        scale_milli: 1000,
        monitors: 1,
        video_codec: VideoCodec::H264,
        audio_codec: None,
        target_fps: 30,
        target_bitrate_kbps: 4000,
    };
    for _ in 0..2 {
        let mut client = RemoteSessionLifecycle::new(connection.clone(), session_id);
        client
            .connect_saved_desktop(&artifact, None, &parameters)
            .await
            .unwrap();
        assert!(connection.state.lock().unwrap().attached);
        client.disconnect(DisconnectMode::Detach).await.unwrap();
        assert!(connection.state.lock().unwrap().retained);
        drop(client);
    }
    {
        let state = connection.state.lock().unwrap();
        assert_eq!(state.resumes, 2);
        assert_eq!(
            state.starts, 0,
            "saved desktops must not send initial resize parameters"
        );
    }
    let _ = tokio::fs::remove_file(binary).await;
}

#[tokio::test]
async fn delayed_end_confirmation_is_observed_while_host_verification_is_unavailable() {
    let binary = std::env::temp_dir().join(format!("ad-agent-{}", Uuid::new_v4()));
    tokio::fs::write(&binary, b"portable agent").await.unwrap();
    let artifact = AgentArtifact::inspect("0.1.0", &binary).await.unwrap();
    let connection = Arc::new(MockConnection::default());
    let session_id = Uuid::new_v4();
    let parameters = SessionParameters {
        session_id,
        desktop_environment: "xfce".into(),
        width: 640,
        height: 400,
        scale_milli: 1000,
        monitors: 1,
        video_codec: VideoCodec::H264,
        audio_codec: None,
        target_fps: 30,
        target_bitrate_kbps: 4000,
    };
    let mut client = RemoteSessionLifecycle::new(connection.clone(), session_id);
    client.connect(&artifact, &parameters).await.unwrap();
    // The old client abandoned the control reply at eight seconds, even
    // though its overall confirmation deadline had not elapsed.
    connection.state.lock().unwrap().end_delay = Duration::from_millis(8250);
    client.disconnect(DisconnectMode::Terminate).await.unwrap();
    assert!(connection.state.lock().unwrap().end_acknowledged);
    client.disconnect(DisconnectMode::Terminate).await.unwrap();
    let _ = tokio::fs::remove_file(binary).await;
}
