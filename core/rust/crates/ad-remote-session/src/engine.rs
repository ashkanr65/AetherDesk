use ad_stream_protocol::{InputMessage, SessionParameters};
use async_trait::async_trait;
use thiserror::Error;

use crate::bootstrap::{AgentArtifact, AgentRuntimeBundle};
use crate::lifecycle::{DisconnectMode, LifecycleError, LifecycleState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Detached,
    Terminated,
    Failed,
}

impl From<LifecycleState> for EngineConnectionState {
    fn from(value: LifecycleState) -> Self {
        match value {
            LifecycleState::Disconnected => Self::Disconnected,
            LifecycleState::Deploying | LifecycleState::Connecting => Self::Connecting,
            LifecycleState::Connected => Self::Connected,
            LifecycleState::Detached => Self::Detached,
            LifecycleState::Terminated => Self::Terminated,
            LifecycleState::Failed => Self::Failed,
        }
    }
}

#[async_trait]
pub trait DisplayEngine: Send {
    type Surface: ?Sized + Sync;
    type RenderStats: Send;

    async fn connect(&mut self) -> Result<(), DisplayEngineError>;
    async fn disconnect(&mut self, mode: DisconnectMode) -> Result<(), DisplayEngineError>;
    async fn handle_input(&self, input: InputMessage) -> Result<(), DisplayEngineError>;
    async fn resize(
        &mut self,
        width: u32,
        height: u32,
        scale_milli: u16,
    ) -> Result<(), DisplayEngineError>;
    fn render_surface(
        &mut self,
        surface: &Self::Surface,
    ) -> Result<Self::RenderStats, DisplayEngineError>;
    fn connection_state(&self) -> EngineConnectionState;
}

#[derive(Clone, Debug)]
pub struct RemoteDesktopConfig {
    pub artifact: AgentArtifact,
    pub runtime: Option<AgentRuntimeBundle>,
    pub parameters: SessionParameters,
}

#[cfg(feature = "rendering")]
mod rendered {
    use std::sync::Arc;
    use std::time::Duration;

    use ad_display_decoder::{CacheLimits, DecoderConfig};
    use ad_display_renderer::{GpuRenderer, RendererConfig, RendererStats};
    use ad_qos_controller::JitterConfig;
    use ad_stream_client::{DemuxedStreams, DisplayStreamEngine, ResumeAction, StreamDemuxer};
    use ad_stream_protocol::{ArppMessage, ControlMessage, DisplayMessage};
    use tokio::time::timeout;

    use super::*;
    use crate::lifecycle::{AgentConnection, RemoteSessionLifecycle};

    pub struct AetherDisplayEngine<C: AgentConnection> {
        config: RemoteDesktopConfig,
        lifecycle: RemoteSessionLifecycle<C>,
        stream: DisplayStreamEngine,
        renderer: GpuRenderer,
        demuxer: Option<StreamDemuxer>,
        channels: Option<DemuxedStreams>,
        pending_clipboard: Vec<ad_stream_protocol::ClipboardMessage>,
        pending_control: Vec<ControlMessage>,
    }

    impl<C: AgentConnection> AetherDisplayEngine<C> {
        #[allow(clippy::too_many_arguments)]
        pub async fn new(
            connection: Arc<C>,
            config: RemoteDesktopConfig,
            instance: &wgpu::Instance,
            compatible_surface: Option<&wgpu::Surface<'_>>,
            target_format: wgpu::TextureFormat,
            decoder: DecoderConfig,
            cache: CacheLimits,
            jitter: JitterConfig,
        ) -> Result<Self, DisplayEngineError> {
            let parameters = &config.parameters;
            let stream = DisplayStreamEngine::new(
                parameters.width,
                parameters.height,
                decoder,
                cache,
                jitter,
            )?;
            let renderer = GpuRenderer::new(
                instance,
                compatible_surface,
                RendererConfig {
                    width: parameters.width,
                    height: parameters.height,
                    target_format,
                    maximum_damage_rectangles: 64,
                },
            )
            .await?;
            Ok(Self {
                lifecycle: RemoteSessionLifecycle::new(connection, parameters.session_id),
                config,
                stream,
                renderer,
                demuxer: None,
                channels: None,
                pending_clipboard: Vec::new(),
                pending_control: Vec::new(),
            })
        }

        pub fn session(&self) -> Option<Arc<ad_stream_client::ArppSession>> {
            self.lifecycle.session()
        }

        pub fn take_clipboard_messages(&mut self) -> Vec<ad_stream_protocol::ClipboardMessage> {
            std::mem::take(&mut self.pending_clipboard)
        }

        pub fn take_control_messages(&mut self) -> Vec<ControlMessage> {
            std::mem::take(&mut self.pending_control)
        }

        /// Drain currently available display/control events. Decoding waits at
        /// most `budget` for each encoded frame and never blocks Flutter's UI thread.
        pub async fn pump(&mut self, budget: Duration) -> Result<usize, DisplayEngineError> {
            let transport_error = self
                .channels
                .as_ref()
                .ok_or(DisplayEngineError::NotConnected)?
                .errors
                .borrow()
                .clone();
            if let Some(error) = transport_error {
                self.lifecycle.mark_transport_lost();
                if let Some(demuxer) = self.demuxer.take() {
                    demuxer.shutdown();
                }
                self.channels = None;
                return Err(DisplayEngineError::TransportLost(error));
            }
            let channels = self
                .channels
                .as_mut()
                .ok_or(DisplayEngineError::NotConnected)?;

            while let Ok(message) = channels.control.try_recv() {
                self.pending_control.push(message);
            }
            while let Ok(message) = channels.clipboard.try_recv() {
                self.pending_clipboard.push(message);
            }

            let mut updates = 0;
            while let Ok(message) = channels.display.try_recv() {
                let encoded = matches!(message, DisplayMessage::EncodedFrame { .. });
                if let Some(update) = self.stream.ingest(message).await? {
                    self.renderer.apply(&update)?;
                    updates += 1;
                }
                if encoded {
                    let update = timeout(budget, self.stream.next_video_update())
                        .await
                        .map_err(|_| DisplayEngineError::DecodeTimeout)??;
                    self.renderer.apply(&update)?;
                    channels
                        .receipts
                        .send_replace(self.stream.resume_snapshot().last_presented_frame);
                    updates += 1;
                }
            }
            Ok(updates)
        }

        fn install_channels(&mut self, session: Arc<ad_stream_client::ArppSession>) {
            if let Some(demuxer) = self.demuxer.take() {
                demuxer.shutdown();
            }
            let (demuxer, channels) = StreamDemuxer::spawn(session);
            self.demuxer = Some(demuxer);
            self.channels = Some(channels);
        }
    }

    #[async_trait]
    impl<C: AgentConnection + 'static> DisplayEngine for AetherDisplayEngine<C> {
        type Surface = wgpu::TextureView;
        type RenderStats = RendererStats;

        async fn connect(&mut self) -> Result<(), DisplayEngineError> {
            let session = if self.lifecycle.state() == LifecycleState::Detached {
                let snapshot = self.stream.resume_snapshot();
                let (session, action) = self.lifecycle.resume(snapshot).await?;
                if let ResumeAction::Accepted {
                    width,
                    height,
                    cache_epoch,
                    full_refresh,
                } = action
                {
                    self.renderer.resize(width, height)?;
                    if full_refresh {
                        let reset = self.stream.flush_for_full_refresh(cache_epoch)?;
                        self.renderer.apply(&reset)?;
                    }
                }
                session
            } else {
                self.lifecycle
                    .connect_bundle(
                        &self.config.artifact,
                        self.config.runtime.as_ref(),
                        &self.config.parameters,
                    )
                    .await?
            };
            self.install_channels(session);
            Ok(())
        }

        async fn disconnect(&mut self, mode: DisconnectMode) -> Result<(), DisplayEngineError> {
            if let Some(mut demuxer) = self.demuxer.take() {
                demuxer.shutdown_and_wait().await;
            }
            self.channels = None;
            self.lifecycle.disconnect(mode).await?;
            Ok(())
        }

        async fn handle_input(&self, input: InputMessage) -> Result<(), DisplayEngineError> {
            self.channels
                .as_ref()
                .ok_or(DisplayEngineError::NotConnected)?
                .send_input(input)
                .await?;
            Ok(())
        }

        async fn resize(
            &mut self,
            width: u32,
            height: u32,
            scale_milli: u16,
        ) -> Result<(), DisplayEngineError> {
            if width == 0
                || height == 0
                || width > 16_384
                || height > 16_384
                || !(250..=4000).contains(&scale_milli)
            {
                return Err(DisplayEngineError::InvalidResize);
            }
            let session = self
                .lifecycle
                .session()
                .ok_or(DisplayEngineError::NotConnected)?;
            session
                .send(ArppMessage::Control(ControlMessage::Resize {
                    width,
                    height,
                    scale_milli,
                }))
                .await?;
            self.renderer.resize(width, height)?;
            Ok(())
        }

        fn render_surface(
            &mut self,
            surface: &Self::Surface,
        ) -> Result<Self::RenderStats, DisplayEngineError> {
            Ok(self.renderer.render_to_view(surface)?)
        }

        fn connection_state(&self) -> EngineConnectionState {
            self.lifecycle.state().into()
        }
    }
}

#[cfg(feature = "rendering")]
pub use rendered::AetherDisplayEngine;

#[derive(Debug, Error)]
pub enum DisplayEngineError {
    #[error("remote session lifecycle failed: {0}")]
    Lifecycle(#[from] LifecycleError),
    #[error("display engine is not connected")]
    NotConnected,
    #[error("remote transport was lost: {0}")]
    TransportLost(String),
    #[error("display resize is invalid")]
    InvalidResize,
    #[error("video decode exceeded the UI frame budget")]
    DecodeTimeout,
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] ad_stream_client::TransportError),
    #[error("ARPP receiver failed: {0}")]
    Receiver(#[from] ad_stream_client::StreamReceiverError),
    #[cfg(feature = "rendering")]
    #[error("display stream failed: {0}")]
    Stream(#[from] ad_stream_client::DisplayStreamError),
    #[cfg(feature = "rendering")]
    #[error("GPU renderer failed: {0}")]
    Renderer(#[from] ad_display_renderer::RendererError),
}
