use std::sync::Arc;
use std::time::Duration;

use ad_stream_client::{
    ArppSession, ResumeAction, ResumeSnapshot, ResumptionManager, TransportAdapter,
};
use ad_stream_protocol::{
    ArppMessage, Capabilities, ControlMessage, SessionParameters, PROTOCOL_VERSION,
};
use async_trait::async_trait;
use ring::rand::{SecureRandom, SystemRandom};
use thiserror::Error;
use tokio::time::timeout;
use uuid::Uuid;

use crate::bootstrap::{
    AgentArtifact, AgentBootstrap, AgentBootstrapError, AgentDeployment, AgentRuntimeBundle,
    RemoteAgentHost,
};

pub const AGENT_CHANNEL_MAGIC: [u8; 4] = *b"ADCH";
pub const AGENT_CHANNEL_VERSION: u8 = 1;
pub const CLOSE_REASON_TERMINATE: u16 = 0;
pub const CLOSE_REASON_DETACH: u16 = 1;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentEndpoint {
    pub port: u16,
    pub token: [u8; 32],
    /// Only a positively identified fresh launch may be rolled back on cancel.
    pub newly_created: bool,
}

#[async_trait]
pub trait AgentConnection: RemoteAgentHost {
    async fn launch_agent(
        &self,
        deployment: &AgentDeployment,
        session_id: Uuid,
        parameters: Option<&SessionParameters>,
    ) -> Result<AgentEndpoint, LifecycleError>;

    async fn open_transport(
        &self,
        endpoint: &AgentEndpoint,
    ) -> Result<Arc<dyn TransportAdapter>, LifecycleError>;

    /// Verify cleanup by agents predating the termination acknowledgement.
    /// Lack of evidence must never be treated as successful termination.
    async fn session_ended(&self, _session_id: Uuid) -> Result<bool, LifecycleError> {
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisconnectMode {
    /// Pause encoding while leaving the compositor and applications alive.
    Detach,
    /// Terminate the compositor, applications, and temporary session files.
    Terminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Disconnected,
    Deploying,
    Connecting,
    Connected,
    Detached,
    Terminated,
    Failed,
}

pub struct RemoteSessionLifecycle<C: AgentConnection> {
    connection: Arc<C>,
    bootstrap: AgentBootstrap<C>,
    session_id: Uuid,
    state: LifecycleState,
    deployment: Option<AgentDeployment>,
    endpoint: Option<AgentEndpoint>,
    session: Option<Arc<ArppSession>>,
    capabilities: Option<Capabilities>,
}

impl<C: AgentConnection> RemoteSessionLifecycle<C> {
    pub fn new(connection: Arc<C>, session_id: Uuid) -> Self {
        Self {
            bootstrap: AgentBootstrap::new(connection.clone()),
            connection,
            session_id,
            state: LifecycleState::Disconnected,
            deployment: None,
            endpoint: None,
            session: None,
            capabilities: None,
        }
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn state(&self) -> LifecycleState {
        self.state
    }
    pub fn newly_created(&self) -> bool {
        self.endpoint.as_ref().is_some_and(|e| e.newly_created)
    }

    pub fn session(&self) -> Option<Arc<ArppSession>> {
        self.session.clone()
    }

    pub fn capabilities(&self) -> Option<&Capabilities> {
        self.capabilities.as_ref()
    }

    pub async fn connect(
        &mut self,
        artifact: &AgentArtifact,
        parameters: &SessionParameters,
    ) -> Result<Arc<ArppSession>, LifecycleError> {
        self.connect_bundle(artifact, None, parameters).await
    }

    pub async fn connect_bundle(
        &mut self,
        artifact: &AgentArtifact,
        runtime: Option<&AgentRuntimeBundle>,
        parameters: &SessionParameters,
    ) -> Result<Arc<ArppSession>, LifecycleError> {
        self.validate_parameters(parameters)?;
        self.state = LifecycleState::Deploying;
        let result = self
            .connect_inner(artifact, runtime, parameters, false, None)
            .await;
        if result.is_err() {
            self.state = LifecycleState::Failed;
        }
        result
    }

    /// Open a saved desktop, keeping applications and dimensions if it already exists.
    pub async fn connect_saved_desktop(
        &mut self,
        artifact: &AgentArtifact,
        runtime: Option<&AgentRuntimeBundle>,
        parameters: &SessionParameters,
    ) -> Result<Arc<ArppSession>, LifecycleError> {
        self.validate_parameters(parameters)?;
        self.state = LifecycleState::Deploying;
        let result = self
            .connect_inner(artifact, runtime, parameters, true, None)
            .await;
        if result.is_err() {
            self.state = LifecycleState::Failed;
        }
        result
    }

    /// Cancellation keeps ownership through launch and closes any partial
    /// handshake. Never terminate a desktop that existed before this attempt.
    pub async fn connect_saved_desktop_cancellable(
        &mut self,
        artifact: &AgentArtifact,
        runtime: Option<&AgentRuntimeBundle>,
        parameters: &SessionParameters,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Arc<ArppSession>, LifecycleError> {
        self.validate_parameters(parameters)?;
        self.state = LifecycleState::Deploying;
        let result = self
            .connect_inner(artifact, runtime, parameters, true, Some(cancel))
            .await;
        if result.is_err() {
            let cleanup = timeout(Duration::from_secs(18), async {
                if self.session.is_none() {
                    if let Some(endpoint) = &self.endpoint {
                        let transport = timeout(
                            Duration::from_secs(5),
                            self.connection.open_transport(endpoint),
                        )
                        .await
                        .map_err(|_| LifecycleError::HandshakeTimeout)??;
                        self.session = Some(Arc::new(ArppSession::new(transport)));
                    }
                }
                let mode = if self.endpoint.as_ref().is_some_and(|e| e.newly_created) {
                    DisconnectMode::Terminate
                } else {
                    DisconnectMode::Detach
                };
                self.disconnect(mode).await
            })
            .await;
            self.state = LifecycleState::Failed;
            match cleanup {
                Ok(Ok(())) => {}
                _ => return Err(LifecycleError::CleanupUnconfirmed(self.session_id)),
            }
        }
        result
    }

    async fn connect_inner(
        &mut self,
        artifact: &AgentArtifact,
        runtime: Option<&AgentRuntimeBundle>,
        parameters: &SessionParameters,
        saved_desktop: bool,
        cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Arc<ArppSession>, LifecycleError> {
        let deployment = cancellable(cancel, async {
            Ok(self
                .bootstrap
                .ensure_bundle_deployed(artifact, runtime)
                .await?)
        })
        .await?;
        self.state = LifecycleState::Connecting;
        // Once launch has started, wait for its bounded response even if the
        // user cancels. Dropping it could lose a newly created agent's identity.
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(LifecycleError::Cancelled);
        }
        let endpoint = timeout(
            Duration::from_secs(35),
            self.connection
                .launch_agent(&deployment, self.session_id, Some(parameters)),
        )
        .await
        .map_err(|_| LifecycleError::CleanupUnconfirmed(self.session_id))??;
        self.endpoint = Some(endpoint.clone());
        self.deployment = Some(deployment.clone());
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(LifecycleError::Cancelled);
        }
        // Finish the bounded three-channel opening before observing cancellation,
        // so cleanup does not race an abandoned, incomplete channel set.
        let transport = timeout(
            Duration::from_secs(10),
            self.connection.open_transport(&endpoint),
        )
        .await
        .map_err(|_| LifecycleError::HandshakeTimeout)??;
        let session = Arc::new(ArppSession::new(transport));
        self.session = Some(session.clone());
        let capabilities = cancellable(cancel, client_hello(&session)).await?;
        validate_capabilities(&capabilities, parameters)?;
        cancellable(cancel, async {
            if saved_desktop {
                session
                    .send(ArppMessage::Control(ControlMessage::ResumeSession {
                        session_id: self.session_id,
                        last_presented_frame: 0,
                        cache_epoch: 0,
                        cache_digest: [0; 32],
                    }))
                    .await?;
                wait_for_resume_state(&session, self.session_id).await?;
            } else {
                session
                    .send(ArppMessage::Control(ControlMessage::StartSession(
                        parameters.clone(),
                    )))
                    .await?;
                wait_for_ready(&session, self.session_id, false).await?;
            }
            Ok(())
        })
        .await?;

        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(LifecycleError::Cancelled);
        }

        self.deployment = Some(deployment);
        self.endpoint = Some(endpoint);
        self.session = Some(session.clone());
        self.capabilities = Some(capabilities);
        self.state = LifecycleState::Connected;
        Ok(session)
    }

    pub async fn resume(
        &mut self,
        snapshot: ResumeSnapshot,
    ) -> Result<(Arc<ArppSession>, ResumeAction), LifecycleError> {
        let deployment = self
            .deployment
            .as_ref()
            .ok_or(LifecycleError::NeverConnected)?;
        self.state = LifecycleState::Connecting;
        let endpoint = self
            .connection
            .launch_agent(deployment, self.session_id, None)
            .await?;
        let transport = self.connection.open_transport(&endpoint).await?;
        let session = Arc::new(ArppSession::new(transport));
        let capabilities = client_hello(&session).await?;
        let manager = ResumptionManager::new(self.session_id, session.clone());
        manager.request(snapshot).await?;
        let state = wait_for_resume_state(&session, self.session_id).await?;
        let action = manager.handle_server_state(&state, snapshot).await?;
        self.endpoint = Some(endpoint);
        self.session = Some(session.clone());
        self.capabilities = Some(capabilities);
        self.state = LifecycleState::Connected;
        Ok((session, action))
    }

    /// Mark an unexpected transport loss. No Close message is emitted, so the
    /// remote supervisor retains the desktop for a later `resume` call.
    pub fn mark_transport_lost(&mut self) {
        self.session = None;
        if self.state != LifecycleState::Terminated {
            self.state = LifecycleState::Detached;
        }
    }

    pub async fn disconnect(&mut self, mode: DisconnectMode) -> Result<(), LifecycleError> {
        let reason = match mode {
            DisconnectMode::Detach => CLOSE_REASON_DETACH,
            DisconnectMode::Terminate => CLOSE_REASON_TERMINATE,
        };
        if mode == DisconnectMode::Terminate
            && self.session.is_none()
            && self.state != LifecycleState::Terminated
        {
            return Err(LifecycleError::TerminationUnconfirmed);
        }
        if let Some(session) = self.session.as_ref() {
            session
                .send(ArppMessage::Control(ControlMessage::Close {
                    reason_code: reason,
                    detail: match mode {
                        DisconnectMode::Detach => "client detached".into(),
                        DisconnectMode::Terminate => "session closed by user".into(),
                    },
                }))
                .await?;
            if mode == DisconnectMode::Terminate {
                // Listen for the authoritative reply for the entire deadline.
                // Legacy host verification runs concurrently: a slow or failed
                // SSH probe must not hide a valid late control confirmation.
                let confirmed = timeout(Duration::from_secs(11), async {
                    let acknowledgement = async {
                        while let Ok(frame) = session.receive().await {
                            if let ArppMessage::Control(ControlMessage::Close {
                                reason_code, ..
                            }) = frame.message
                            {
                                if reason_code == CLOSE_REASON_TERMINATE {
                                    return;
                                }
                                break;
                            }
                        }
                        // EOF alone cannot prove that the desktop was stopped.
                        std::future::pending::<()>().await;
                    };
                    let host_verification = async {
                        // Give modern agents time to reply without an SSH probe.
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        loop {
                            if self
                                .connection
                                .session_ended(self.session_id)
                                .await
                                .unwrap_or(false)
                            {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                    };
                    tokio::select! {
                        _ = acknowledgement => {},
                        _ = host_verification => {},
                    }
                })
                .await;
                if confirmed.is_err() {
                    return Err(LifecycleError::TerminationUnconfirmed);
                }
            }
            if mode == DisconnectMode::Terminate {
                // Host termination is confirmed. Best-effort channel cleanup
                // must not turn it into a failure at the outer FFI deadline.
                let _ = timeout(
                    Duration::from_secs(1),
                    session.close("AetherDesk session ended"),
                )
                .await;
            } else {
                session.close("AetherDesk client detached").await?;
            }
        }
        self.session = None;
        self.state = match mode {
            DisconnectMode::Detach => LifecycleState::Detached,
            DisconnectMode::Terminate => {
                self.endpoint = None;
                LifecycleState::Terminated
            }
        };
        Ok(())
    }

    fn validate_parameters(&self, parameters: &SessionParameters) -> Result<(), LifecycleError> {
        if parameters.session_id != self.session_id {
            return Err(LifecycleError::SessionMismatch);
        }
        if parameters.width == 0
            || parameters.height == 0
            || parameters.width > 16_384
            || parameters.height > 16_384
            || parameters.monitors == 0
            || parameters.target_fps == 0
            || parameters.target_fps > 240
            || parameters.desktop_environment.is_empty()
            || parameters.desktop_environment.len() > 64
        {
            return Err(LifecycleError::InvalidParameters);
        }
        Ok(())
    }
}

async fn cancellable<T>(
    cancel: Option<&tokio_util::sync::CancellationToken>,
    operation: impl std::future::Future<Output = Result<T, LifecycleError>>,
) -> Result<T, LifecycleError> {
    tokio::select! {
        biased;
        _ = async { match cancel { Some(c) => c.cancelled().await, None => std::future::pending().await } } => Err(LifecycleError::Cancelled),
        result = operation => result,
    }
}

async fn client_hello(session: &Arc<ArppSession>) -> Result<Capabilities, LifecycleError> {
    let mut nonce = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| LifecycleError::Random)?;
    session
        .send(ArppMessage::Control(ControlMessage::ClientHello {
            protocol_version: PROTOCOL_VERSION,
            client_nonce: nonce,
        }))
        .await?;

    let mut server_hello = false;
    let mut capabilities = None;
    for _ in 0..8 {
        let frame = timeout(HANDSHAKE_TIMEOUT, session.receive())
            .await
            .map_err(|_| LifecycleError::HandshakeTimeout)??;
        match frame.message {
            ArppMessage::Control(ControlMessage::ServerHello {
                protocol_version, ..
            }) => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(LifecycleError::ProtocolVersion {
                        client: PROTOCOL_VERSION,
                        agent: protocol_version,
                    });
                }
                server_hello = true;
            }
            ArppMessage::Control(ControlMessage::Capabilities(value)) => capabilities = Some(value),
            _ => return Err(LifecycleError::UnexpectedHandshakeMessage),
        }
        if server_hello {
            if let Some(capabilities) = capabilities {
                return Ok(capabilities);
            }
        }
    }
    Err(LifecycleError::UnexpectedHandshakeMessage)
}

fn validate_capabilities(
    capabilities: &Capabilities,
    parameters: &SessionParameters,
) -> Result<(), LifecycleError> {
    if capabilities.protocol_version != PROTOCOL_VERSION {
        return Err(LifecycleError::ProtocolVersion {
            client: PROTOCOL_VERSION,
            agent: capabilities.protocol_version,
        });
    }
    if parameters.width > capabilities.maximum_width
        || parameters.height > capabilities.maximum_height
        || parameters.monitors > capabilities.maximum_monitors
        || !capabilities.video_codecs.contains(&parameters.video_codec)
        || (parameters.audio_codec.is_some() && !capabilities.audio)
    {
        return Err(LifecycleError::UnsupportedParameters);
    }
    Ok(())
}

async fn wait_for_ready(
    session: &Arc<ArppSession>,
    session_id: Uuid,
    expected_resumed: bool,
) -> Result<(), LifecycleError> {
    let frame = timeout(HANDSHAKE_TIMEOUT, session.receive())
        .await
        .map_err(|_| LifecycleError::HandshakeTimeout)??;
    match frame.message {
        ArppMessage::Control(ControlMessage::SessionReady {
            session_id: returned,
            resumed,
        }) if returned == session_id && resumed == expected_resumed => Ok(()),
        _ => Err(LifecycleError::UnexpectedHandshakeMessage),
    }
}

async fn wait_for_resume_state(
    session: &Arc<ArppSession>,
    session_id: Uuid,
) -> Result<ControlMessage, LifecycleError> {
    let frame = timeout(HANDSHAKE_TIMEOUT, session.receive())
        .await
        .map_err(|_| LifecycleError::HandshakeTimeout)??;
    match frame.message {
        ArppMessage::Control(message @ ControlMessage::ResumeState { session_id: id, .. })
            if id == session_id =>
        {
            Ok(message)
        }
        _ => Err(LifecycleError::UnexpectedHandshakeMessage),
    }
}

pub fn channel_prelude(channel: u8, token: &[u8; 32]) -> Result<[u8; 38], LifecycleError> {
    if channel > 2 || token.iter().all(|value| *value == 0) {
        return Err(LifecycleError::InvalidEndpoint);
    }
    let mut output = [0_u8; 38];
    output[..4].copy_from_slice(&AGENT_CHANNEL_MAGIC);
    output[4] = AGENT_CHANNEL_VERSION;
    output[5] = channel;
    output[6..].copy_from_slice(token);
    Ok(output)
}

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error("Connection cancelled")]
    Cancelled,
    #[error(
        "Startup cleanup could not be confirmed. Reconnect to retained desktop {0} to check it."
    )]
    CleanupUnconfirmed(Uuid),
    #[error("agent deployment failed: {0}")]
    Bootstrap(#[from] AgentBootstrapError),
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] ad_stream_client::TransportError),
    #[error("ARPP resumption failed: {0}")]
    Resumption(#[from] ad_stream_client::ResumptionError),
    #[error("the session parameters use another session identifier")]
    SessionMismatch,
    #[error("the session parameters are invalid")]
    InvalidParameters,
    #[error("the agent does not support the requested parameters")]
    UnsupportedParameters,
    #[error("client protocol {client} is incompatible with agent protocol {agent}")]
    ProtocolVersion { client: u16, agent: u16 },
    #[error("agent handshake timed out")]
    HandshakeTimeout,
    #[error("agent sent an unexpected handshake message")]
    UnexpectedHandshakeMessage,
    #[error("secure random generation failed")]
    Random,
    #[error("agent endpoint is invalid")]
    InvalidEndpoint,
    #[error("the remote session has never connected")]
    NeverConnected,
    #[error(
        "The host did not confirm that the remote session ended. Reconnect to check its state."
    )]
    TerminationUnconfirmed,
    #[error("agent launch failed: {0}")]
    Launch(String),
}
