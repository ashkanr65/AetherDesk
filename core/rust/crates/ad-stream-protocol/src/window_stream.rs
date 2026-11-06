//! Selected-window mirror over an authenticated SSH exec channel. This does not
//! change ARPP v3 or connect applications to a different X server.
use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{lossless::LosslessTile, Rectangle};

pub const MAGIC: &[u8; 8] = b"ADWIN002";
pub const MAX_PACKET: usize = 36 * 1024 * 1024;
pub const MAX_PIXELS: usize = 8_388_608;
pub const MAX_WINDOWS: usize = 256;
pub const MAX_TITLEBAR_HEIGHT: u32 = 256;

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
    /// Explicit user command, executed only inside this private host session.
    Launch(String),
    /// Ordered input only; no nested batches, capture or process operations.
    InputBatch(Vec<WindowInput>),
    /// Opt in to unsolicited, ordered image/metadata updates. Also a lease heartbeat.
    Stream {
        active: bool,
    },
    /// Opt in before selection; old runtimes can be retried without this extension.
    Titlebar {
        enabled: bool,
    },
    /// A complete primary click in the last displayed host decoration strip.
    TitlebarClick {
        window: u32,
        x: u32,
        y: u32,
    },
    /// Graceful close for a client without window-manager decorations.
    CloseApplication {
        window: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WindowInput {
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
}
impl TryFrom<Request> for WindowInput {
    type Error = ();
    fn try_from(request: Request) -> Result<Self, ()> {
        Ok(match request {
            Request::Focus => Self::Focus,
            Request::Resize { width, height } => Self::Resize { width, height },
            Request::Key { evdev, pressed } => Self::Key { evdev, pressed },
            Request::Move { x, y } => Self::Move { x, y },
            Request::Button {
                button,
                pressed,
                x,
                y,
            } => Self::Button {
                button,
                pressed,
                x,
                y,
            },
            Request::Wheel {
                horizontal,
                vertical,
            } => Self::Wheel {
                horizontal,
                vertical,
            },
            Request::Release => Self::Release,
            _ => return Err(()),
        })
    }
}
impl From<WindowInput> for Request {
    fn from(input: WindowInput) -> Self {
        match input {
            WindowInput::Focus => Self::Focus,
            WindowInput::Resize { width, height } => Self::Resize { width, height },
            WindowInput::Key { evdev, pressed } => Self::Key { evdev, pressed },
            WindowInput::Move { x, y } => Self::Move { x, y },
            WindowInput::Button {
                button,
                pressed,
                x,
                y,
            } => Self::Button {
                button,
                pressed,
                x,
                y,
            },
            WindowInput::Wheel {
                horizontal,
                vertical,
            } => Self::Wheel {
                horizontal,
                vertical,
            },
            WindowInput::Release => Self::Release,
        }
    }
}

pub fn batchable(request: &Request) -> bool {
    matches!(
        request,
        Request::Focus
            | Request::Resize { .. }
            | Request::Key { .. }
            | Request::Move { .. }
            | Request::Button { .. }
            | Request::Wheel { .. }
            | Request::Release
    )
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
    /// The same selected application changed its title or dimensions.
    Metadata(WindowInfo),
    /// A stream's selected view ended; input errors alone do not close a view.
    ViewClosed(String),
    /// Independently bounded, full decoration image for the selected client.
    Titlebar {
        window: u32,
        image: Option<Popup>,
    },
}

pub fn validate_titlebar(image: &Popup) -> Result<(), &'static str> {
    dimensions(image.width, image.height)?;
    if image.window == 0
        || image.height > MAX_TITLEBAR_HEIGHT
        || image.x < 0
        || image.x > 1024
        || image.y != 0
    {
        return Err("Invalid application title bar");
    }
    Ok(())
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
    let decoded = decode_tiles(
        previous.map(|(s, w, h, _)| (s, w, h)),
        sequence,
        base,
        width,
        height,
        tiles,
    )?;
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
    for (region, bytes) in decoded {
        for row in 0..region.height as usize {
            let target = ((region.y as usize + row) * width as usize + region.x as usize) * 4;
            let stride = region.width as usize * 4;
            pixels[target..target + stride]
                .copy_from_slice(&bytes[row * stride..(row + 1) * stride]);
        }
    }
    Ok(pixels)
}

/// Validate and decode a complete transaction before publishing any changed tile.
/// Consumers can retain immutable unchanged tiles instead of copying the desktop.
pub fn decode_tiles(
    previous: Option<(u64, u32, u32)>,
    sequence: u64,
    base: Option<u64>,
    width: u32,
    height: u32,
    tiles: Vec<LosslessTile>,
) -> Result<Vec<(Rectangle, Vec<u8>)>, String> {
    dimensions(width, height)?;
    if sequence == 0 {
        return Err("Invalid application frame sequence".into());
    }
    if let Some(base) = base {
        if !previous
            .is_some_and(|(seq, w, h)| seq == base && sequence > seq && w == width && h == height)
        {
            return Err("Application frame base is stale".into());
        }
    }
    let columns = width.div_ceil(256);
    let rows = height.div_ceil(256);
    if tiles.len() > (columns * rows) as usize {
        return Err("Too many application tiles".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut decoded = Vec::with_capacity(tiles.len());
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
        let region = tile.region;
        let bytes = tile.decode()?;
        decoded.push((region, bytes));
    }
    if base.is_none() && seen.len() != (columns * rows) as usize {
        return Err("Incomplete application image".into());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_titlebar_packets_are_opt_in_bounded_and_separate_from_view_stop() {
        for request in [
            Request::Titlebar { enabled: true },
            Request::TitlebarClick {
                window: 8,
                x: 90,
                y: 12,
            },
            Request::CloseApplication { window: 8 },
        ] {
            assert!(!batchable(&request));
            assert!(WindowInput::try_from(request.clone()).is_err());
            assert_eq!(
                encode(&decode::<Request>(&encode(&request).unwrap()).unwrap()).unwrap(),
                encode(&request).unwrap()
            );
        }
        assert!(matches!(
            decode::<Request>(&encode(&Request::Stop).unwrap()).unwrap(),
            Request::Stop
        ));
        let mut image = Popup {
            window: 99,
            x: 4,
            y: 0,
            width: 800,
            height: 24,
            tiles: Vec::new(),
        };
        validate_titlebar(&image).unwrap();
        image.height = MAX_TITLEBAR_HEIGHT + 1;
        assert!(validate_titlebar(&image).is_err());
        image.height = 24;
        image.x = -1;
        assert!(validate_titlebar(&image).is_err());
        image.x = 0;
        image.window = 0;
        assert!(validate_titlebar(&image).is_err());
    }
    #[test]
    fn streaming_extensions_preserve_legacy_discriminants_and_metadata() {
        assert_eq!(encode(&Request::Poll).unwrap(), 2u32.to_le_bytes());
        assert_eq!(encode(&Reply::Ready).unwrap(), 0u32.to_le_bytes());
        let subscribe = encode(&Request::Stream { active: false }).unwrap();
        assert_eq!(&subscribe[..4], &13u32.to_le_bytes());
        assert!(matches!(
            decode::<Request>(&subscribe).unwrap(),
            Request::Stream { active: false }
        ));
        assert!(WindowInput::try_from(Request::Stream { active: true }).is_err());
        let info = WindowInfo {
            id: 99,
            title: "Virtuoso® Studio — Layout".into(),
            width: 800,
            height: 600,
            transient_for: None,
        };
        let reply = decode::<Reply>(&encode(&Reply::Metadata(info.clone())).unwrap()).unwrap();
        assert!(matches!(reply,Reply::Metadata(value) if value==info));
    }
    #[test]
    fn ordered_input_batch_round_trips_without_nested_or_launch_commands() {
        let batch = Request::InputBatch(vec![
            WindowInput::Focus,
            WindowInput::Key {
                evdev: 42,
                pressed: true,
            },
            WindowInput::Button {
                button: 1,
                pressed: true,
                x: 15,
                y: 25,
            },
            WindowInput::Button {
                button: 1,
                pressed: false,
                x: 15,
                y: 25,
            },
            WindowInput::Release,
        ]);
        let bytes = encode(&batch).unwrap();
        let Request::InputBatch(input) = decode::<Request>(&bytes).unwrap() else {
            panic!("input batch changed wire identity")
        };
        assert_eq!(input.len(), 5);
        assert!(matches!(
            input[1],
            WindowInput::Key {
                evdev: 42,
                pressed: true
            }
        ));
        assert!(matches!(input[4], WindowInput::Release));
        assert!(WindowInput::try_from(Request::Launch("xterm".into())).is_err());
        assert!(WindowInput::try_from(Request::InputBatch(input)).is_err());
    }
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
