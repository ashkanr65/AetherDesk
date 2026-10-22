//! Selected-window mirror over an authenticated SSH exec channel. This does not
//! change ARPP v3 or connect applications to a different X server.
use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{lossless::LosslessTile, Rectangle};

pub const MAGIC: &[u8; 8] = b"ADWIN002";
pub const MAX_PACKET: usize = 36 * 1024 * 1024;
pub const MAX_PIXELS: usize = 8_388_608;
pub const MAX_WINDOWS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub transient_for: Option<u32>,
}

/// A complete lossless menu image, positioned relative to the selected client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Popup {
    pub window: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub tiles: Vec<LosslessTile>,
}
pub const MAX_POPUPS: usize = 16;

pub fn validate_popups(popups: &[Popup]) -> Result<(), &'static str> {
    if popups.len() > MAX_POPUPS {
        return Err("Too many application menus");
    }
    let mut pixels = 0_u64;
    let mut ids = std::collections::BTreeSet::new();
    for p in popups {
        dimensions(p.width, p.height)?;
        if p.window == 0
            || !ids.insert(p.window)
            || p.x.unsigned_abs() > 32767
            || p.y.unsigned_abs() > 32767
        {
            return Err("Invalid application menu identity or position");
        }
        pixels += u64::from(p.width) * u64::from(p.height);
        if pixels > MAX_PIXELS as u64 {
            return Err("Application menus exceed the capture budget");
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Select(u32),
    Poll,
    Focus,
    Resize {
        width: u32,
        height: u32,
    },
    Key {
        evdev: u32,
        pressed: bool,
    },
    Move {
        x: i32,
        y: i32,
    },
    Button {
        button: u8,
        pressed: bool,
        x: i32,
        y: i32,
    },
    Wheel {
        horizontal: i32,
        vertical: i32,
    },
    Release,
    Stop,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Reply {
    Ready,
    Windows(Vec<WindowInfo>),
    Selected(WindowInfo),
    Frame {
        window: u32,
        sequence: u64,
        base: Option<u64>,
        width: u32,
        height: u32,
        tiles: Vec<LosslessTile>,
        popups: Vec<Popup>,
    },
    Idle,
    Ack,
    Error(String),
}

pub fn dimensions(width: u32, height: u32) -> Result<usize, &'static str> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || width > 8192 || height > 8192 || pixels > MAX_PIXELS as u64 {
        return Err("Application window exceeds the supported capture size");
    }
    Ok(pixels as usize * 4)
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_PACKET as u64)
        .reject_trailing_bytes()
        .serialize(value)
        .map_err(|_| "Cannot encode window message".into())
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() > MAX_PACKET {
        return Err("Window message exceeds the size limit".into());
    }
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_PACKET as u64)
        .reject_trailing_bytes()
        .deserialize(bytes)
        .map_err(|_| "Invalid window message".into())
}

/// Publish only a complete validated image. An invalid transaction cannot alter
/// the last good image or allocate from a compressed embedded size.
pub fn apply_frame(
    previous: Option<(u64, u32, u32, &[u8])>,
    sequence: u64,
    base: Option<u64>,
    width: u32,
    height: u32,
    tiles: Vec<LosslessTile>,
) -> Result<Vec<u8>, String> {
    let count = dimensions(width, height)?;
    if sequence == 0 {
        return Err("Invalid application frame sequence".into());
    }
    let mut pixels = if let Some(base) = base {
        match previous {
            Some((seq, w, h, bytes))
                if seq == base
                    && sequence > seq
                    && w == width
                    && h == height
                    && bytes.len() == count =>
            {
                bytes.to_vec()
            }
            _ => return Err("Application frame base is stale".into()),
        }
    } else {
        vec![0; count]
    };
    let columns = width.div_ceil(256);
    let rows = height.div_ceil(256);
    if tiles.len() > (columns * rows) as usize {
        return Err("Too many application tiles".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for tile in tiles {
        let Rectangle {
            x,
            y,
            width: w,
            height: h,
        } = tile.region;
        if x >= width
            || y >= height
            || x % 256 != 0
            || y % 256 != 0
            || w != (width - x).min(256)
            || h != (height - y).min(256)
            || !seen.insert((x, y))
        {
            return Err("Invalid or duplicate application tile".into());
        }
        let bytes = tile.decode()?;
        for row in 0..h as usize {
            let target = ((y as usize + row) * width as usize + x as usize) * 4;
            pixels[target..target + w as usize * 4]
                .copy_from_slice(&bytes[row * w as usize * 4..(row + 1) * w as usize * 4]);
        }
    }
    if base.is_none() && seen.len() != (columns * rows) as usize {
        return Err("Incomplete application image".into());
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_packets_preserve_signed_offsets_and_reject_unbounded_or_duplicate_layers() {
        let popup = Popup {
            window: 2,
            x: -20,
            y: 40,
            width: 1,
            height: 1,
            tiles: vec![LosslessTile::encode(
                Rectangle {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                vec![7, 21, 240, 255],
            )
            .unwrap()],
        };
        validate_popups(&[popup.clone()]).unwrap();
        assert!(validate_popups(&[popup.clone(), popup.clone()]).is_err());
        assert!(validate_popups(&vec![popup.clone(); MAX_POPUPS + 1]).is_err());
        let mut invalid = popup.clone();
        invalid.width = u32::MAX;
        assert!(validate_popups(&[invalid]).is_err());
        let frame = Reply::Frame {
            window: 1,
            sequence: 2,
            base: Some(1),
            width: 1,
            height: 1,
            tiles: Vec::new(),
            popups: vec![popup.clone()],
        };
        let decoded: Reply = decode(&encode(&frame).unwrap()).unwrap();
        assert!(matches!(decoded,Reply::Frame{popups,..}if popups==vec![popup]));
    }
    #[test]
    fn window_stream_preserves_exact_pixels_and_rejects_partial_or_stale_images() {
        let tile = LosslessTile::encode(
            Rectangle {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
            vec![1, 2, 254, 255, 255, 0, 7, 255],
        )
        .unwrap();
        let pixels = apply_frame(None, 1, None, 2, 1, vec![tile.clone()]).unwrap();
        assert_eq!(pixels, vec![1, 2, 254, 255, 255, 0, 7, 255]);
        assert!(apply_frame(None, 1, None, 2, 1, vec![]).is_err());
        assert!(apply_frame(None, 1, None, 2, 1, vec![tile.clone(), tile.clone()]).is_err());
        assert!(apply_frame(Some((1, 2, 1, &pixels)), 2, Some(9), 2, 1, vec![tile]).is_err());
        assert_eq!(
            apply_frame(Some((1, 2, 1, &pixels)), 2, Some(1), 2, 1, vec![]).unwrap(),
            pixels
        );
        assert!(dimensions(u32::MAX, 1).is_err());
        let encoded = encode(&Request::Select(42)).unwrap();
        assert!(matches!(
            decode::<Request>(&encoded).unwrap(),
            Request::Select(42)
        ));
        assert!(decode::<Request>(&encoded[..encoded.len() - 1]).is_err());
    }
}
