//! Immutable, structurally shared BGRA snapshots. This is local storage only;
//! it does not change the wire format or pixel values.
use crate::{lossless::TILE_EDGE, Rectangle};
use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug)]
pub struct PixelTile {
    pub region: Rectangle,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct TileSnapshot {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub tiles: Vec<Arc<PixelTile>>,
}

#[derive(Debug)]
pub struct PixelBuffer {
    flat: OnceLock<Arc<Vec<u8>>>,
    tiles: Option<TileSnapshot>,
}
impl Clone for PixelBuffer {
    fn clone(&self) -> Self {
        // Do not copy a materialized screenshot back into the incremental path.
        let flat = OnceLock::new();
        if self.tiles.is_none() {
            if let Some(bytes) = self.flat.get() {
                let _ = flat.set(bytes.clone());
            }
        }
        Self {
            flat,
            tiles: self.tiles.clone(),
        }
    }
}
impl From<Vec<u8>> for PixelBuffer {
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            flat: OnceLock::from(Arc::new(bytes)),
            tiles: None,
        }
    }
}
impl PixelBuffer {
    pub fn len(&self) -> usize {
        self.tiles.as_ref().map_or_else(
            || self.flat.get().map_or(0, |p| p.len()),
            |t| t.stride * t.height as usize,
        )
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn tiles(&self) -> Option<&TileSnapshot> {
        self.tiles.as_ref()
    }
    pub fn flat(&self) -> Arc<Vec<u8>> {
        self.materialize().clone()
    }
    fn materialize(&self) -> &Arc<Vec<u8>> {
        self.flat.get_or_init(|| {
            let Some(t) = &self.tiles else {
                return Arc::new(vec![]);
            };
            let mut bytes = vec![0; t.stride * t.height as usize];
            for tile in &t.tiles {
                let r = tile.region;
                let row_bytes = r.width as usize * 4;
                for row in 0..r.height as usize {
                    let start = (r.y as usize + row) * t.stride + r.x as usize * 4;
                    bytes[start..start + row_bytes]
                        .copy_from_slice(&tile.bytes[row * row_bytes..(row + 1) * row_bytes]);
                }
            }
            Arc::new(bytes)
        })
    }
    /// Capture only damaged tiles, reusing the rest from the previous complete
    /// snapshot. Arc identity tracks changes across ANY number of skipped frames.
    pub fn capture(
        width: u32,
        height: u32,
        stride: usize,
        bytes: &[u8],
        damage: &[Rectangle],
        previous: Option<&Self>,
    ) -> Result<Self, &'static str> {
        validate_size(width, height, stride, bytes.len())?;
        let previous = previous
            .and_then(Self::tiles)
            .filter(|p| (p.width, p.height, p.stride) == (width, height, stride));
        let valid_damage = damage.iter().all(|r| valid_region(*r, width, height));
        let mut tiles = Vec::new();
        for y in (0..height).step_by(TILE_EDGE as usize) {
            for x in (0..width).step_by(TILE_EDGE as usize) {
                let r = Rectangle {
                    x,
                    y,
                    width: TILE_EDGE.min(width - x),
                    height: TILE_EDGE.min(height - y),
                };
                let old = previous.and_then(|p| p.tiles.get(tiles.len()));
                if let Some(old) = old {
                    if valid_damage && !damage.iter().any(|d| overlaps(*d, r)) {
                        tiles.push(old.clone());
                        continue;
                    }
                    if (0..r.height as usize).all(|row| {
                        let a = (y as usize + row) * stride + x as usize * 4;
                        let b = row * r.width as usize * 4;
                        bytes[a..a + r.width as usize * 4] == old.bytes[b..b + r.width as usize * 4]
                    }) {
                        tiles.push(old.clone());
                        continue;
                    }
                }
                let mut pixels = Vec::with_capacity(r.width as usize * r.height as usize * 4);
                for row in 0..r.height as usize {
                    let start = (y as usize + row) * stride + x as usize * 4;
                    pixels.extend_from_slice(&bytes[start..start + r.width as usize * 4]);
                }
                tiles.push(Arc::new(PixelTile {
                    region: r,
                    bytes: pixels,
                }));
            }
        }
        Ok(Self {
            flat: OnceLock::new(),
            tiles: Some(TileSnapshot {
                width,
                height,
                stride,
                tiles,
            }),
        })
    }
    pub fn blank(width: u32, height: u32) -> Result<Self, &'static str> {
        let stride = width as usize * 4;
        validate_size(
            width,
            height,
            stride,
            stride.saturating_mul(height as usize),
        )?;
        let mut tiles = Vec::new();
        for y in (0..height).step_by(TILE_EDGE as usize) {
            for x in (0..width).step_by(TILE_EDGE as usize) {
                let r = Rectangle {
                    x,
                    y,
                    width: TILE_EDGE.min(width - x),
                    height: TILE_EDGE.min(height - y),
                };
                tiles.push(Arc::new(PixelTile {
                    region: r,
                    bytes: [0, 0, 0, 255].repeat(r.width as usize * r.height as usize),
                }));
            }
        }
        Ok(Self {
            flat: OnceLock::new(),
            tiles: Some(TileSnapshot {
                width,
                height,
                stride,
                tiles,
            }),
        })
    }
    /// Validated lossless transactions use complete, grid-aligned tiles. No full
    /// framebuffer clone is needed to keep previous published images immutable.
    pub fn replace_tile(
        &mut self,
        width: u32,
        height: u32,
        region: Rectangle,
        bytes: Vec<u8>,
    ) -> Result<(), &'static str> {
        if !valid_region(region, width, height)
            || !region.x.is_multiple_of(TILE_EDGE)
            || !region.y.is_multiple_of(TILE_EDGE)
            || region.width != TILE_EDGE.min(width - region.x)
            || region.height != TILE_EDGE.min(height - region.y)
            || bytes.len() != region.width as usize * region.height as usize * 4
        {
            return Err("Invalid snapshot tile");
        }
        if self
            .tiles
            .as_ref()
            .is_none_or(|t| (t.width, t.height, t.stride) != (width, height, width as usize * 4))
        {
            *self = Self::capture(width, height, width as usize * 4, self, &[], None)?;
        }
        let t = self.tiles.as_mut().ok_or("Missing snapshot tiles")?;
        let index = (region.y / TILE_EDGE) as usize * width.div_ceil(TILE_EDGE) as usize
            + (region.x / TILE_EDGE) as usize;
        if t.tiles[index].bytes != bytes {
            t.tiles[index] = Arc::new(PixelTile { region, bytes });
        }
        self.flat = OnceLock::new();
        Ok(())
    }
}
fn validate_size(width: u32, height: u32, stride: usize, len: usize) -> Result<(), &'static str> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 33_554_432
        || stride < width as usize * 4
        || stride.checked_mul(height as usize).is_none_or(|n| n > len)
    {
        return Err("Invalid pixel snapshot");
    }
    Ok(())
}
fn valid_region(r: Rectangle, width: u32, height: u32) -> bool {
    r.width > 0
        && r.height > 0
        && r.x.checked_add(r.width).is_some_and(|n| n <= width)
        && r.y.checked_add(r.height).is_some_and(|n| n <= height)
}
fn overlaps(a: Rectangle, b: Rectangle) -> bool {
    u64::from(a.x) < u64::from(b.x) + u64::from(b.width)
        && u64::from(b.x) < u64::from(a.x) + u64::from(a.width)
        && u64::from(a.y) < u64::from(b.y) + u64::from(b.height)
        && u64::from(b.y) < u64::from(a.y) + u64::from(a.height)
}
impl Deref for PixelBuffer {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        self.materialize()
    }
}
impl DerefMut for PixelBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.materialize();
        self.tiles = None;
        Arc::make_mut(self.flat.get_mut().expect("materialized pixels"))
    }
}
impl PartialEq<Vec<u8>> for PixelBuffer {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.deref() == other
    }
}
impl PartialEq<PixelBuffer> for Vec<u8> {
    fn eq(&self, other: &PixelBuffer) -> bool {
        self == other.deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_tiles_preserve_skipped_changes_and_materialize_only_on_demand() {
        let mut raw = [7, 21, 240, 255].repeat(1024 * 512);
        let first = PixelBuffer::capture(1024, 512, 4096, &raw, &[], None).unwrap();
        raw[..4].copy_from_slice(&[1, 2, 3, 255]);
        let second = PixelBuffer::capture(
            1024,
            512,
            4096,
            &raw,
            &[Rectangle {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }],
            Some(&first),
        )
        .unwrap();
        raw[1024 * 4..1024 * 4 + 4].copy_from_slice(&[4, 5, 6, 255]);
        let third = PixelBuffer::capture(
            1024,
            512,
            4096,
            &raw,
            &[Rectangle {
                x: 0,
                y: 1,
                width: 1,
                height: 1,
            }],
            Some(&second),
        )
        .unwrap();
        assert!(!Arc::ptr_eq(
            &first.tiles().unwrap().tiles[0],
            &third.tiles().unwrap().tiles[0]
        ));
        for index in 1..8 {
            assert!(Arc::ptr_eq(
                &first.tiles().unwrap().tiles[index],
                &third.tiles().unwrap().tiles[index]
            ));
        }
        assert!(third.flat.get().is_none());
        assert_eq!(&third[..4], &[1, 2, 3, 255]);
        assert_eq!(&third[4096..4100], &[4, 5, 6, 255]);
        assert_eq!(&first[..4], &[7, 21, 240, 255]);
        let mut patched = third.clone();
        patched
            .replace_tile(
                1024,
                512,
                Rectangle {
                    x: 256,
                    y: 256,
                    width: 256,
                    height: 256,
                },
                [12, 34, 56, 255].repeat(256 * 256),
            )
            .unwrap();
        assert!(patched.flat.get().is_none());
        assert!(Arc::ptr_eq(
            &third.tiles().unwrap().tiles[0],
            &patched.tiles().unwrap().tiles[0]
        ));
        assert_eq!(
            &patched[(256 * 1024 + 256) * 4..(256 * 1024 + 256) * 4 + 4],
            &[12, 34, 56, 255]
        );
        assert_eq!(
            &third[(256 * 1024 + 256) * 4..(256 * 1024 + 256) * 4 + 4],
            &[7, 21, 240, 255]
        );
    }
    #[test]
    fn resize_invalid_damage_and_padded_rows_fall_back_safely() {
        let first = PixelBuffer::blank(257, 2).unwrap();
        let stride = 257 * 4 + 16;
        let raw = [11_u8; (257 * 4 + 16) * 2];
        let next = PixelBuffer::capture(
            257,
            2,
            stride,
            &raw,
            &[Rectangle {
                x: u32::MAX,
                y: 0,
                width: 9,
                height: 1,
            }],
            Some(&first),
        )
        .unwrap();
        assert_eq!(next.len(), stride * 2);
        assert_eq!(&next[..257 * 4], &raw[..257 * 4]);
        assert_eq!(
            &next[stride..stride + 257 * 4],
            &raw[stride..stride + 257 * 4]
        );
        assert!(PixelBuffer::capture(257, 2, stride, &raw[..9], &[], None).is_err());
        let mut mutable = first.clone();
        mutable[0] = 99;
        assert!(mutable.tiles().is_none());
        assert_eq!(first[0], 0);
        assert!(mutable
            .replace_tile(
                257,
                2,
                Rectangle {
                    x: 1,
                    y: 0,
                    width: 256,
                    height: 2
                },
                vec![0; 2048]
            )
            .is_err());
    }
}
