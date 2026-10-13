//! Renderer-neutral display contracts shared by RDP and Aether Stream.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PixelFormat {
    Bgra8888,
    Rgba8888,
    Nv12,
}

impl PixelFormat {
    pub const fn minimum_bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra8888 | Self::Rgba8888 => 4,
            Self::Nv12 => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceFrame {
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
    pub damage: Vec<DamageRect>,
    pub pixels: Vec<u8>,
}

impl SurfaceFrame {
    pub fn validate(&self) -> Result<(), DisplayError> {
        if self.width == 0 || self.height == 0 {
            return Err(DisplayError::InvalidDimensions);
        }
        let minimum_stride = (self.width as usize)
            .checked_mul(self.format.minimum_bytes_per_pixel())
            .ok_or(DisplayError::FrameTooLarge)?;
        if self.stride < minimum_stride {
            return Err(DisplayError::InvalidStride {
                actual: self.stride,
                minimum: minimum_stride,
            });
        }
        let minimum_length = self
            .stride
            .checked_mul(self.height as usize)
            .ok_or(DisplayError::FrameTooLarge)?;
        if self.pixels.len() < minimum_length {
            return Err(DisplayError::IncompleteFrame {
                actual: self.pixels.len(),
                minimum: minimum_length,
            });
        }
        for rectangle in &self.damage {
            let right = rectangle
                .x
                .checked_add(rectangle.width)
                .ok_or(DisplayError::InvalidDamage)?;
            let bottom = rectangle
                .y
                .checked_add(rectangle.height)
                .ok_or(DisplayError::InvalidDamage)?;
            if rectangle.width == 0
                || rectangle.height == 0
                || right > self.width
                || bottom > self.height
            {
                return Err(DisplayError::InvalidDamage);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DisplayInput {
    Key {
        scan_code: u32,
        pressed: bool,
        extended: bool,
    },
    Pointer {
        x: u32,
        y: u32,
        buttons: u16,
    },
    Wheel {
        delta_x: i32,
        delta_y: i32,
    },
    Resize {
        width: u32,
        height: u32,
        scale_percent: u32,
    },
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CursorShape {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClipboardPayload {
    Text(String),
    Files(Vec<String>),
}

pub trait SurfaceSink: Send + Sync {
    fn submit_frame(&self, frame: SurfaceFrame) -> Result<(), DisplayError>;
    fn update_cursor(&self, cursor: CursorShape) -> Result<(), DisplayError>;
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum DisplayError {
    #[error("display dimensions must be non-zero")]
    InvalidDimensions,
    #[error("frame dimensions overflow the address space")]
    FrameTooLarge,
    #[error("frame stride {actual} is smaller than required stride {minimum}")]
    InvalidStride { actual: usize, minimum: usize },
    #[error("frame contains {actual} bytes but requires at least {minimum}")]
    IncompleteFrame { actual: usize, minimum: usize },
    #[error("damage rectangle is outside the surface")]
    InvalidDamage,
    #[error("display consumer is no longer available")]
    ConsumerClosed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> SurfaceFrame {
        SurfaceFrame {
            sequence: 1,
            width: 2,
            height: 2,
            stride: 8,
            format: PixelFormat::Bgra8888,
            damage: vec![DamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }],
            pixels: vec![0; 16],
        }
    }

    #[test]
    fn accepts_a_complete_frame() {
        assert_eq!(frame().validate(), Ok(()));
    }

    #[test]
    fn rejects_out_of_bounds_damage() {
        let mut value = frame();
        value.damage[0].x = 1;
        assert_eq!(value.validate(), Err(DisplayError::InvalidDamage));
    }

    #[test]
    fn rejects_short_pixel_storage() {
        let mut value = frame();
        value.pixels.pop();
        assert_eq!(
            value.validate(),
            Err(DisplayError::IncompleteFrame {
                actual: 15,
                minimum: 16
            })
        );
    }
}
