use std::time::Instant;

use ad_display_decoder::{
    CacheLimits, DecoderConfig, DecoderError, PresentationError, PresentationUpdate,
    RemotePresentation, VideoDecoder,
};
use ad_qos_controller::{JitterBuffer, JitterConfig, JitterError};
use ad_stream_protocol::DisplayMessage;
use thiserror::Error;

use crate::ResumeSnapshot;

pub struct DisplayStreamEngine {
    decoder: VideoDecoder,
    presentation: RemotePresentation,
    jitter: JitterBuffer<ad_display_decoder::DecodedFrame>,
}

impl DisplayStreamEngine {
    pub fn new(
        width: u32,
        height: u32,
        decoder: DecoderConfig,
        cache: CacheLimits,
        jitter: JitterConfig,
    ) -> Result<Self, DisplayStreamError> {
        Ok(Self {
            decoder: VideoDecoder::new(decoder)?,
            presentation: RemotePresentation::new(width, height, cache)?,
            jitter: JitterBuffer::new(jitter)?,
        })
    }

    pub async fn ingest(
        &mut self,
        message: DisplayMessage,
    ) -> Result<Option<PresentationUpdate>, DisplayStreamError> {
        match self.presentation.handle(message)? {
            PresentationUpdate::Decode { message, damage } => {
                self.decoder.submit(&message, damage).await?;
                Ok(None)
            }
            PresentationUpdate::None => Ok(None),
            update => Ok(Some(update)),
        }
    }

    pub async fn next_video_update(&mut self) -> Result<PresentationUpdate, DisplayStreamError> {
        let decoded = self.decoder.next_frame().await?;
        self.jitter.push(decoded, Instant::now());
        loop {
            let now = Instant::now();
            if let Some(frame) = self.jitter.pop_ready(now) {
                return Ok(self.presentation.accept_decoded(frame)?);
            }
            let deadline = self
                .jitter
                .next_deadline()
                .ok_or(DisplayStreamError::JitterEmpty)?;
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        }
    }

    pub fn resume_snapshot(&self) -> ResumeSnapshot {
        ResumeSnapshot {
            last_presented_frame: self.presentation.last_presented_frame(),
            cache_epoch: self.presentation.cache_epoch(),
            cache_digest: self.presentation.cache_digest(),
        }
    }

    pub fn flush_for_full_refresh(
        &mut self,
        cache_epoch: u64,
    ) -> Result<PresentationUpdate, DisplayStreamError> {
        self.decoder.flush()?;
        self.jitter.clear();
        Ok(self
            .presentation
            .handle(DisplayMessage::CacheReset { epoch: cache_epoch })?)
    }
}

#[derive(Debug, Error)]
pub enum DisplayStreamError {
    #[error("display decoder failed: {0}")]
    Decoder(#[from] DecoderError),
    #[error("presentation reconstruction failed: {0}")]
    Presentation(#[from] PresentationError),
    #[error("jitter buffer failed: {0}")]
    Jitter(#[from] JitterError),
    #[error("jitter buffer unexpectedly became empty")]
    JitterEmpty,
}
