use std::collections::BTreeMap;

use ad_stream_protocol::{DisplayMessage, PixelFormat, Rectangle};
use thiserror::Error;

use crate::{CacheEntry, CacheError, CacheLimits, DecodedFrame, PresentationCache};

#[derive(Clone, Debug, Default)]
pub struct CursorState {
    pub frame_id: u64,
    pub x: i32,
    pub y: i32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum PresentationUpdate {
    Decode {
        message: DisplayMessage,
        damage: Vec<Rectangle>,
    },
    BaseLayer {
        frame_id: u64,
        width: u32,
        height: u32,
        pixels_bgra: Vec<u8>,
        damage: Vec<Rectangle>,
    },
    BasePatch {
        frame_id: u64,
        region: Rectangle,
        stride: usize,
        pixels_bgra: Vec<u8>,
    },
    VideoLayer(DecodedFrame),
    Cursor(CursorState),
    CacheReset(u64),
    None,
}

pub struct RemotePresentation {
    width: u32,
    height: u32,
    pending_damage: BTreeMap<u64, Vec<Rectangle>>,
    cache: PresentationCache,
    cursor: CursorState,
    last_presented_frame: u64,
}

impl RemotePresentation {
    pub fn new(width: u32, height: u32, limits: CacheLimits) -> Result<Self, PresentationError> {
        validate_dimensions(width, height)?;
        Ok(Self {
            width,
            height,
            pending_damage: BTreeMap::new(),
            cache: PresentationCache::new(limits)?,
            cursor: CursorState::default(),
            last_presented_frame: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn cache_epoch(&self) -> u64 {
        self.cache.epoch()
    }
    pub fn cache_digest(&self) -> [u8; 32] {
        self.cache.digest()
    }
    pub fn last_presented_frame(&self) -> u64 {
        self.last_presented_frame
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), PresentationError> {
        validate_dimensions(width, height)?;
        self.width = width;
        self.height = height;
        self.pending_damage.clear();
        Ok(())
    }

    pub fn handle(
        &mut self,
        message: DisplayMessage,
    ) -> Result<PresentationUpdate, PresentationError> {
        match message {
            DisplayMessage::DamageRegions { frame_id, regions } => {
                validate_regions(self.width, self.height, &regions)?;
                // Damage metadata may arrive ahead of its encoded frame. Keep
                // this state bounded if a peer drops frames or sends hostile
                // frame identifiers; a missing entry safely becomes a full
                // frame redraw when the encoded frame arrives.
                if self.pending_damage.len() >= 64 && !self.pending_damage.contains_key(&frame_id) {
                    self.pending_damage.pop_first();
                }
                self.pending_damage.insert(frame_id, regions);
                Ok(PresentationUpdate::None)
            }
            encoded @ DisplayMessage::EncodedFrame {
                frame_id,
                width,
                height,
                ..
            } => {
                if width != self.width || height != self.height {
                    self.resize(width, height)?;
                }
                let damage = self.pending_damage.remove(&frame_id).unwrap_or_else(|| {
                    vec![Rectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }]
                });
                Ok(PresentationUpdate::Decode {
                    message: encoded,
                    damage,
                })
            }
            DisplayMessage::TileUpdate {
                frame_id,
                region,
                format,
                data,
                ..
            } => {
                let entry = CacheEntry::new(region.width, region.height, format, data)?;
                self.last_presented_frame = self.last_presented_frame.max(frame_id);
                self.patch_update(frame_id, region, &entry)
            }
            DisplayMessage::CacheStore {
                frame_id,
                cache_key,
                namespace,
                region,
                format,
                data,
            } => {
                let entry = CacheEntry::new(region.width, region.height, format, data)?;
                self.cache.insert(namespace, cache_key, entry)?;
                let entry = self
                    .cache
                    .get(namespace, &cache_key)
                    .ok_or(PresentationError::CacheMiss)?;
                self.last_presented_frame = self.last_presented_frame.max(frame_id);
                self.patch_update(frame_id, region, &entry)
            }
            DisplayMessage::CacheReference {
                frame_id,
                cache_key,
                namespace,
                region,
            } => {
                let entry = self
                    .cache
                    .get(namespace, &cache_key)
                    .ok_or(PresentationError::CacheMiss)?;
                if entry.width != region.width || entry.height != region.height {
                    return Err(PresentationError::CacheDimensions);
                }
                self.last_presented_frame = self.last_presented_frame.max(frame_id);
                self.patch_update(frame_id, region, &entry)
            }
            DisplayMessage::CacheReset { epoch } => {
                self.cache.reset(epoch);
                self.last_presented_frame = 0;
                Ok(PresentationUpdate::CacheReset(epoch))
            }
            DisplayMessage::CursorUpdate {
                frame_id,
                x,
                y,
                hotspot_x,
                hotspot_y,
                width,
                height,
                visible,
                rgba,
            } => {
                let expected = (width as usize)
                    .checked_mul(height as usize)
                    .and_then(|pixels| pixels.checked_mul(4))
                    .ok_or(PresentationError::Dimensions)?;
                if (visible && (width == 0 || height == 0 || rgba.len() != expected))
                    || hotspot_x >= width.max(1)
                    || hotspot_y >= height.max(1)
                {
                    return Err(PresentationError::Cursor);
                }
                self.cursor = CursorState {
                    frame_id,
                    x,
                    y,
                    hotspot_x,
                    hotspot_y,
                    width,
                    height,
                    visible,
                    rgba,
                };
                Ok(PresentationUpdate::Cursor(self.cursor.clone()))
            }
        }
    }

    pub fn accept_decoded(
        &mut self,
        frame: DecodedFrame,
    ) -> Result<PresentationUpdate, PresentationError> {
        if frame.width != self.width || frame.height != self.height {
            self.resize(frame.width, frame.height)?;
        }
        validate_regions(frame.width, frame.height, &frame.damage)?;
        let expected = frame
            .stride
            .checked_mul(frame.height as usize)
            .ok_or(PresentationError::Dimensions)?;
        if frame.stride < frame.width as usize * 4 || frame.pixels_bgra.len() < expected {
            return Err(PresentationError::DecodedFrame);
        }
        self.last_presented_frame = self.last_presented_frame.max(frame.frame_id);
        Ok(PresentationUpdate::VideoLayer(frame))
    }

    fn patch_update(
        &self,
        frame_id: u64,
        region: Rectangle,
        entry: &CacheEntry,
    ) -> Result<PresentationUpdate, PresentationError> {
        validate_regions(self.width, self.height, &[region])?;
        if entry.width != region.width || entry.height != region.height {
            return Err(PresentationError::CacheDimensions);
        }
        Ok(PresentationUpdate::BasePatch {
            frame_id,
            region,
            stride: region.width as usize * 4,
            pixels_bgra: to_bgra(entry)?,
        })
    }
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), PresentationError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(PresentationError::Dimensions);
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(PresentationError::Dimensions)?;
    Ok(())
}

fn validate_regions(
    width: u32,
    height: u32,
    regions: &[Rectangle],
) -> Result<(), PresentationError> {
    for region in regions {
        if region.width == 0
            || region.height == 0
            || region
                .x
                .checked_add(region.width)
                .is_none_or(|right| right > width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err(PresentationError::Damage);
        }
    }
    Ok(())
}

fn to_bgra(entry: &CacheEntry) -> Result<Vec<u8>, PresentationError> {
    match entry.format {
        PixelFormat::Bgra8888 => Ok(entry.data.to_vec()),
        PixelFormat::Rgba8888 => {
            let mut output = entry.data.to_vec();
            for pixel in output.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Ok(output)
        }
        PixelFormat::Nv12 => nv12_to_bgra(entry),
    }
}

fn nv12_to_bgra(entry: &CacheEntry) -> Result<Vec<u8>, PresentationError> {
    let width = entry.width as usize;
    let height = entry.height as usize;
    if !width.is_multiple_of(2) || !height.is_multiple_of(2) {
        return Err(PresentationError::Nv12Dimensions);
    }
    let y_size = width * height;
    let mut output = vec![0_u8; y_size * 4];
    for y in 0..height {
        for x in 0..width {
            let luma = entry.data[y * width + x] as f32;
            let uv = y_size + (y / 2) * width + (x / 2) * 2;
            let u = entry.data[uv] as f32 - 128.0;
            let v = entry.data[uv + 1] as f32 - 128.0;
            let r = (luma + 1.402 * v).clamp(0.0, 255.0) as u8;
            let g = (luma - 0.344_136 * u - 0.714_136 * v).clamp(0.0, 255.0) as u8;
            let b = (luma + 1.772 * u).clamp(0.0, 255.0) as u8;
            let offset = (y * width + x) * 4;
            output[offset..offset + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    Ok(output)
}

#[derive(Debug, Error)]
pub enum PresentationError {
    #[error("presentation dimensions are invalid")]
    Dimensions,
    #[error("damage region is outside the presentation")]
    Damage,
    #[error("cache reference is unavailable")]
    CacheMiss,
    #[error("cache entry does not match its destination")]
    CacheDimensions,
    #[error("cursor payload is invalid")]
    Cursor,
    #[error("decoded frame storage is invalid")]
    DecodedFrame,
    #[error("NV12 cache entries require even dimensions")]
    Nv12Dimensions,
    #[error("presentation cache failed: {0}")]
    Cache(#[from] CacheError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ad_stream_protocol::CacheNamespace;

    #[test]
    fn stores_and_reuses_content_addressed_tiles() {
        let mut state = RemotePresentation::new(4, 4, CacheLimits::default()).unwrap();
        let entry = CacheEntry::new(2, 2, PixelFormat::Bgra8888, vec![7; 16]).unwrap();
        let key = entry.content_key(CacheNamespace::Tile);
        state
            .handle(DisplayMessage::CacheStore {
                frame_id: 1,
                cache_key: key,
                namespace: CacheNamespace::Tile,
                region: Rectangle {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
                format: PixelFormat::Bgra8888,
                data: entry.data.to_vec(),
            })
            .unwrap();
        let update = state
            .handle(DisplayMessage::CacheReference {
                frame_id: 2,
                cache_key: key,
                namespace: CacheNamespace::Tile,
                region: Rectangle {
                    x: 2,
                    y: 2,
                    width: 2,
                    height: 2,
                },
            })
            .unwrap();
        assert!(matches!(
            update,
            PresentationUpdate::BasePatch { frame_id: 2, .. }
        ));
    }

    #[test]
    fn reset_changes_the_resumption_digest() {
        let mut state = RemotePresentation::new(2, 2, CacheLimits::default()).unwrap();
        let before = state.cache_digest();
        state
            .handle(DisplayMessage::CacheReset { epoch: 4 })
            .unwrap();
        assert_ne!(before, state.cache_digest());
        assert_eq!(state.cache_epoch(), 4);
    }

    #[test]
    fn pending_damage_metadata_is_bounded() {
        let mut state = RemotePresentation::new(2, 2, CacheLimits::default()).unwrap();
        for frame_id in 0..100 {
            state
                .handle(DisplayMessage::DamageRegions {
                    frame_id,
                    regions: vec![Rectangle {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    }],
                })
                .unwrap();
        }
        assert_eq!(state.pending_damage.len(), 64);
        assert!(!state.pending_damage.contains_key(&0));
        assert!(state.pending_damage.contains_key(&99));
    }
}
