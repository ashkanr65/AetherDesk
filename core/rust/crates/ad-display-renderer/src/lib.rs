//! GPU compositor for decoded Aether Stream presentation layers.

mod renderer;

pub use renderer::{GpuRenderer, RendererConfig, RendererError, RendererStats};
