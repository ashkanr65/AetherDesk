//! ARPP video decoding and presentation-state reconstruction.

mod cache;
mod framebuffer;
pub use framebuffer::Framebuffer;
mod decoder;
mod state;

pub use cache::{CacheEntry, CacheError, CacheLimits, PresentationCache};
pub use decoder::{
    DecodedFrame, DecoderBackend, DecoderConfig, DecoderError, DecoderSelection, VideoDecoder,
};
pub use state::{CursorState, PresentationError, PresentationUpdate, RemotePresentation};
