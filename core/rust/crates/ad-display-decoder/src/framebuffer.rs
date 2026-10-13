//! CPU presentation surface for native texture hosts that own their GPU device.

use crate::{CursorState, PresentationError, PresentationUpdate};
use std::sync::Arc;

/// A bounded BGRA surface. Flutter's native texture host uploads snapshots.
pub struct Framebuffer {
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
    cursor: CursorState,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Result<Self, PresentationError> {
        let len = checked_size(width, height)?;
        let mut pixels = vec![0; len];
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(pixels),
            cursor: CursorState::default(),
        })
    }

    /// Transfer tightly packed decoded frames without a second full-frame copy.
    pub fn apply_owned(&mut self, update: PresentationUpdate) -> Result<bool, PresentationError> {
        if let PresentationUpdate::VideoLayer(frame) = update {
            let len = checked_size(frame.width, frame.height)?;
            if frame.stride == frame.width as usize * 4 && frame.pixels_bgra.len() == len {
                self.width = frame.width;
                self.height = frame.height;
                self.pixels = Arc::new(frame.pixels_bgra);
                return Ok(true);
            }
            return self.apply(&PresentationUpdate::VideoLayer(frame));
        }
        self.apply(&update)
    }

    pub fn apply(&mut self, update: &PresentationUpdate) -> Result<bool, PresentationError> {
        match update {
            PresentationUpdate::VideoLayer(frame) => {
                if (self.width, self.height) != (frame.width, frame.height) {
                    *self = Self::new(frame.width, frame.height)?;
                }
                self.patch(
                    0,
                    0,
                    frame.width,
                    frame.height,
                    frame.stride,
                    &frame.pixels_bgra,
                )?;
            }
            PresentationUpdate::BaseLayer {
                width,
                height,
                pixels_bgra,
                ..
            } => {
                if (self.width, self.height) != (*width, *height) {
                    *self = Self::new(*width, *height)?;
                }
                self.patch(0, 0, *width, *height, *width as usize * 4, pixels_bgra)?;
            }
            PresentationUpdate::BasePatch {
                region,
                stride,
                pixels_bgra,
                ..
            } => {
                self.patch(
                    region.x,
                    region.y,
                    region.width,
                    region.height,
                    *stride,
                    pixels_bgra,
                )?;
            }
            PresentationUpdate::Cursor(cursor) => {
                self.cursor = cursor.clone();
            }
            PresentationUpdate::CacheReset(_) => {
                *self = Self::new(self.width, self.height)?;
            }
            PresentationUpdate::None | PresentationUpdate::Decode { .. } => return Ok(false),
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn patch(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        stride: usize,
        data: &[u8],
    ) -> Result<(), PresentationError> {
        if x.checked_add(width).is_none_or(|end| end > self.width)
            || y.checked_add(height).is_none_or(|end| end > self.height)
            || stride < width as usize * 4
            || stride
                .checked_mul(height as usize)
                .is_none_or(|len| len > data.len())
        {
            return Err(PresentationError::DecodedFrame);
        }
        let pixels = Arc::make_mut(&mut self.pixels);
        for row in 0..height as usize {
            let start = ((y as usize + row) * self.width as usize + x as usize) * 4;
            pixels[start..start + width as usize * 4]
                .copy_from_slice(&data[row * stride..row * stride + width as usize * 4]);
        }
        Ok(())
    }

    /// Return a snapshot with the cursor composited without damaging its backing pixels.
    pub fn snapshot(&self) -> (u32, u32, Arc<Vec<u8>>) {
        let mut pixels = self.pixels.clone();
        let cursor = &self.cursor;
        if cursor.visible
            && (cursor.width as usize)
                .checked_mul(cursor.height as usize)
                .and_then(|n| n.checked_mul(4))
                == Some(cursor.rgba.len())
        {
            let pixels = Arc::make_mut(&mut pixels);
            for row in 0..cursor.height as usize {
                for col in 0..cursor.width as usize {
                    let x = i64::from(cursor.x) - i64::from(cursor.hotspot_x) + col as i64;
                    let y = i64::from(cursor.y) - i64::from(cursor.hotspot_y) + row as i64;
                    if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
                        continue;
                    }
                    let src = (row * cursor.width as usize + col) * 4;
                    let dst = (y as usize * self.width as usize + x as usize) * 4;
                    let alpha = u32::from(cursor.rgba[src + 3]);
                    for (target, source) in [(0, 2), (1, 1), (2, 0)] {
                        pixels[dst + target] = ((u32::from(cursor.rgba[src + source]) * alpha
                            + u32::from(pixels[dst + target]) * (255 - alpha)
                            + 127)
                            / 255) as u8;
                    }
                    pixels[dst + 3] = 255;
                }
            }
        }
        (self.width, self.height, pixels)
    }
}

fn checked_size(width: u32, height: u32) -> Result<usize, PresentationError> {
    let size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if width == 0
        || height == 0
        || width > 16_384
        || height > 16_384
        || u64::from(width) * u64::from(height) > 33_554_432
    {
        return Err(PresentationError::Dimensions);
    }
    size.ok_or(PresentationError::Dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DecodedFrame;
    use ad_stream_protocol::Rectangle;

    #[test]
    fn padded_rows_and_patches_preserve_other_pixels() {
        let mut buffer = Framebuffer::new(2, 2).unwrap();
        let frame = DecodedFrame {
            frame_id: 1,
            presentation_time_us: 0,
            width: 2,
            height: 2,
            stride: 12,
            damage: vec![],
            pixels_bgra: vec![
                1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, 7, 8, 9, 255, 10, 11, 12, 255, 9, 9, 9, 9,
            ],
        };
        buffer
            .apply(&PresentationUpdate::VideoLayer(frame))
            .unwrap();
        buffer
            .apply(&PresentationUpdate::BasePatch {
                frame_id: 2,
                region: Rectangle {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
                stride: 4,
                pixels_bgra: vec![20, 21, 22, 255],
            })
            .unwrap();
        assert_eq!(
            buffer.snapshot().2.as_slice(),
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 20, 21, 22, 255]
        );
    }

    #[test]
    fn cursor_clips_and_does_not_burn_into_backing_frame() {
        let mut buffer = Framebuffer::new(1, 1).unwrap();
        buffer
            .apply(&PresentationUpdate::Cursor(CursorState {
                x: -1,
                y: 0,
                width: 2,
                height: 1,
                visible: true,
                rgba: vec![0, 255, 0, 255, 255, 0, 0, 255],
                ..Default::default()
            }))
            .unwrap();
        assert_eq!(buffer.snapshot().2.as_slice(), [0, 0, 255, 255]);
        buffer
            .apply(&PresentationUpdate::Cursor(CursorState::default()))
            .unwrap();
        assert_eq!(buffer.snapshot().2.as_slice(), [0, 0, 0, 255]);
    }

    #[test]
    fn snapshots_share_storage_and_remain_immutable_after_patches() {
        let mut buffer = Framebuffer::new(1, 1).unwrap();
        let snapshot = buffer.snapshot().2;
        assert!(Arc::ptr_eq(&snapshot, &buffer.snapshot().2));
        buffer.patch(0, 0, 1, 1, 4, &[1, 2, 3, 255]).unwrap();
        assert_eq!(snapshot.as_slice(), [0, 0, 0, 255]);
        assert_eq!(buffer.snapshot().2.as_slice(), [1, 2, 3, 255]);
    }

    #[test]
    fn rejects_invalid_dimensions_and_out_of_bounds_patches() {
        assert!(Framebuffer::new(u32::MAX, 1).is_err());
        let mut buffer = Framebuffer::new(1, 1).unwrap();
        assert!(buffer.patch(u32::MAX, 0, 1, 1, 4, &[0; 4]).is_err());
        assert!(buffer.patch(0, 0, 1, 1, 4, &[]).is_err());
    }
}
