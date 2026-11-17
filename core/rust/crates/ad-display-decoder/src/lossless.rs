//! Assemble a bounded transaction before exposing any pixels to presentation.
use std::collections::HashSet;

use ad_stream_protocol::{
    lossless::{TileEncoding, MAX_FRAME_BYTES, MAX_TILES, TILE_EDGE},
    DisplayMessage, Rectangle,
};

#[derive(Debug)]
pub struct LosslessPatch {
    pub region: Rectangle,
    pub pixels_bgra: Vec<u8>,
    pub copy_from: Option<(u32, u32)>,
}

#[derive(Debug)]
pub struct LosslessUpdate {
    pub frame_id: u64,
    pub base_frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub patches: Vec<LosslessPatch>,
}

struct Pending {
    update: LosslessUpdate,
    expected: usize,
    bytes: usize,
    positions: HashSet<(u32, u32)>,
}

#[derive(Default)]
pub struct LosslessAssembler {
    pending: Option<Pending>,
    committed: u64,
    dimensions: (u32, u32),
}

impl LosslessAssembler {
    pub fn handles(message: &DisplayMessage) -> bool {
        matches!(
            message,
            DisplayMessage::LosslessBegin { .. }
                | DisplayMessage::LosslessTile { .. }
                | DisplayMessage::LosslessCommit { .. }
        )
    }

    pub fn push(
        &mut self,
        message: DisplayMessage,
    ) -> Result<Option<LosslessUpdate>, &'static str> {
        match message {
            DisplayMessage::LosslessBegin {
                frame_id,
                base_frame_id,
                width,
                height,
                tile_count,
            } => {
                if !ad_stream_protocol::valid_desktop_size(width, height)
                    || frame_id <= self.committed
                    || tile_count == 0
                    || tile_count as usize > MAX_TILES
                    || (base_frame_id != 0
                        && (base_frame_id != self.committed || self.dimensions != (width, height)))
                    || self.pending.is_some()
                {
                    return Err("Invalid lossless frame start or base");
                }
                self.pending = Some(Pending {
                    update: LosslessUpdate {
                        frame_id,
                        base_frame_id,
                        width,
                        height,
                        patches: Vec::new(),
                    },
                    expected: tile_count as usize,
                    bytes: 0,
                    positions: HashSet::new(),
                });
                Ok(None)
            }
            DisplayMessage::LosslessTile { frame_id, tile } => {
                let pending = self
                    .pending
                    .as_mut()
                    .ok_or("Lossless tile without frame start")?;
                let region = tile.region;
                let update = &mut pending.update;
                if frame_id != update.frame_id
                    || update.patches.len() >= pending.expected
                    || region.x >= update.width
                    || region.y >= update.height
                    || !region.x.is_multiple_of(TILE_EDGE)
                    || !region.y.is_multiple_of(TILE_EDGE)
                    || region.width != TILE_EDGE.min(update.width - region.x)
                    || region.height != TILE_EDGE.min(update.height - region.y)
                    || !pending.positions.insert((region.x, region.y))
                {
                    return Err("Invalid, duplicate or overlapping lossless tile");
                }
                let (pixels_bgra, copy_from) =
                    if let TileEncoding::CopyBgra { x, y } = tile.encoding {
                        if update.base_frame_id == 0
                            || !tile.data.is_empty()
                            || !x.is_multiple_of(TILE_EDGE)
                            || !y.is_multiple_of(TILE_EDGE)
                            || x.checked_add(region.width).is_none_or(|n| n > update.width)
                            || y.checked_add(region.height)
                                .is_none_or(|n| n > update.height)
                            || region.width != TILE_EDGE.min(update.width - x)
                            || region.height != TILE_EDGE.min(update.height - y)
                        {
                            return Err("Invalid lossless tile copy source");
                        }
                        (Vec::new(), Some((x, y)))
                    } else {
                        (tile.decode()?, None)
                    };
                pending.bytes = pending
                    .bytes
                    .checked_add(region.width as usize * region.height as usize * 4)
                    .ok_or("Lossless frame overflow")?;
                if pending.bytes > MAX_FRAME_BYTES {
                    return Err("Oversized lossless frame");
                }
                update.patches.push(LosslessPatch {
                    region,
                    pixels_bgra,
                    copy_from,
                });
                Ok(None)
            }
            DisplayMessage::LosslessCommit { frame_id } => {
                let pending = self
                    .pending
                    .take()
                    .ok_or("Lossless commit without frame start")?;
                if frame_id != pending.update.frame_id
                    || pending.update.patches.len() != pending.expected
                {
                    return Err("Incomplete lossless transaction");
                }
                self.committed = frame_id;
                self.dimensions = (pending.update.width, pending.update.height);
                Ok(Some(pending.update))
            }
            _ => Err("Unexpected message inside lossless transaction"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Framebuffer;
    use ad_stream_protocol::lossless::LosslessTile;

    const DESKTOP_HEIGHT: u32 = 64;

    fn base_for_copy() -> (LosslessAssembler, Framebuffer) {
        let mut assembler = LosslessAssembler::default();
        let mut framebuffer = Framebuffer::new(512, 64).unwrap();
        assembler.push(begin(1, 0, 512, 2)).unwrap();
        for (x, colour) in [(0, [9_u8, 18, 27, 255]), (256, [64, 128, 192, 255])] {
            let tile = LosslessTile::encode(
                Rectangle {
                    x,
                    y: 0,
                    width: 256,
                    height: 64,
                },
                colour.repeat(256 * 64),
            )
            .unwrap();
            assembler
                .push(DisplayMessage::LosslessTile { frame_id: 1, tile })
                .unwrap();
        }
        framebuffer
            .apply_lossless(
                assembler
                    .push(DisplayMessage::LosslessCommit { frame_id: 1 })
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        (assembler, framebuffer)
    }

    #[test]
    fn copied_tiles_resolve_from_one_immutable_base_and_swaps_are_atomic() {
        let (mut assembler, mut framebuffer) = base_for_copy();
        let original = framebuffer.snapshot_shared().2;
        assembler.push(begin(2, 1, 512, 2)).unwrap();
        for (x, source) in [(0, 256), (256, 0)] {
            assembler
                .push(DisplayMessage::LosslessTile {
                    frame_id: 2,
                    tile: LosslessTile {
                        region: Rectangle {
                            x,
                            y: 0,
                            width: 256,
                            height: 64,
                        },
                        encoding: TileEncoding::CopyBgra { x: source, y: 0 },
                        data: Vec::new(),
                    },
                })
                .unwrap();
        }
        let update = assembler
            .push(DisplayMessage::LosslessCommit { frame_id: 2 })
            .unwrap()
            .unwrap();
        assert_eq!(&framebuffer.snapshot().2[..4], &[9, 18, 27, 255]);
        framebuffer.apply_lossless(update).unwrap();
        let pixels = framebuffer.snapshot().2;
        assert_eq!(&pixels[..4], &[64, 128, 192, 255]);
        assert_eq!(&pixels[256 * 4..256 * 4 + 4], &[9, 18, 27, 255]);
        assert_eq!(&original[..4], &[9, 18, 27, 255]);
    }

    #[test]
    fn copied_tiles_reject_missing_bases_payloads_and_invalid_source_regions() {
        for (base, x, y, data) in [
            (0, 0, 0, vec![]),
            (1, 512, 0, vec![]),
            (1, 1, 0, vec![]),
            (1, 0, u32::MAX, vec![]),
            (1, 0, 0, vec![1]),
        ] {
            let (mut assembler, framebuffer) = base_for_copy();
            assembler.push(begin(2, base, 512, 1)).unwrap();
            let before = framebuffer.snapshot().2;
            assert!(assembler
                .push(DisplayMessage::LosslessTile {
                    frame_id: 2,
                    tile: LosslessTile {
                        region: Rectangle {
                            x: 0,
                            y: 0,
                            width: 256,
                            height: 64
                        },
                        encoding: TileEncoding::CopyBgra { x, y },
                        data
                    }
                })
                .is_err());
            assert_eq!(framebuffer.snapshot().2, before);
        }
    }

    fn begin(frame_id: u64, base_frame_id: u64, width: u32, count: u32) -> DisplayMessage {
        DisplayMessage::LosslessBegin {
            frame_id,
            base_frame_id,
            width,
            height: DESKTOP_HEIGHT,
            tile_count: count,
        }
    }
    fn tile(frame_id: u64, x: u32, colour: [u8; 4]) -> DisplayMessage {
        DisplayMessage::LosslessTile {
            frame_id,
            tile: LosslessTile::encode(
                Rectangle {
                    x,
                    y: 0,
                    width: 256,
                    height: DESKTOP_HEIGHT,
                },
                colour.repeat(256 * DESKTOP_HEIGHT as usize),
            )
            .unwrap(),
        }
    }

    #[test]
    fn lossless_transactions_are_atomic_exact_and_preserve_unchanged_monitors() {
        let mut assembler = LosslessAssembler::default();
        let mut buffer = Framebuffer::new(512, DESKTOP_HEIGHT).unwrap();
        let original = buffer.snapshot().2;
        assert!(assembler.push(begin(1, 0, 512, 2)).unwrap().is_none());
        assert!(assembler
            .push(tile(1, 0, [7, 21, 240, 255]))
            .unwrap()
            .is_none());
        assert_eq!(buffer.snapshot().2, original);
        assembler.push(tile(1, 256, [233, 17, 99, 255])).unwrap();
        buffer
            .apply_lossless(
                assembler
                    .push(DisplayMessage::LosslessCommit { frame_id: 1 })
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let first = buffer.snapshot().2;
        assert_eq!(&first[..4], &[7, 21, 240, 255]);
        assert_eq!(&first[256 * 4..256 * 4 + 4], &[233, 17, 99, 255]);
        assembler.push(begin(2, 1, 512, 1)).unwrap();
        assembler.push(tile(2, 0, [11, 22, 33, 255])).unwrap();
        buffer
            .apply_lossless(
                assembler
                    .push(DisplayMessage::LosslessCommit { frame_id: 2 })
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(&buffer.snapshot().2[..4], &[11, 22, 33, 255]);
        assert_eq!(
            &buffer.snapshot().2[256 * 4..256 * 4 + 4],
            &[233, 17, 99, 255]
        );
        assert_eq!(
            &first[..4],
            &[7, 21, 240, 255],
            "published screenshots must stay immutable"
        );
        assert!(
            assembler.push(begin(3, 1, 512, 1)).is_err(),
            "stale base accepted"
        );
        assembler.push(begin(3, 0, 256, 1)).unwrap();
        assembler.push(tile(3, 0, [42, 43, 44, 255])).unwrap();
        buffer
            .apply_lossless(
                assembler
                    .push(DisplayMessage::LosslessCommit { frame_id: 3 })
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(buffer.snapshot().0, 256);
        // A two-pixel edge tile is legal; a two-pixel desktop is not.
        assembler.push(begin(4, 0, 258, 2)).unwrap();
        assembler.push(tile(4, 0, [10, 20, 30, 255])).unwrap();
        assembler
            .push(DisplayMessage::LosslessTile {
                frame_id: 4,
                tile: LosslessTile::encode(
                    Rectangle {
                        x: 256,
                        y: 0,
                        width: 2,
                        height: DESKTOP_HEIGHT,
                    },
                    [40, 50, 60, 255].repeat(2 * DESKTOP_HEIGHT as usize),
                )
                .unwrap(),
            })
            .unwrap();
        buffer
            .apply_lossless(
                assembler
                    .push(DisplayMessage::LosslessCommit { frame_id: 4 })
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        let (width, height, pixels) = buffer.snapshot();
        assert_eq!((width, height), (258, DESKTOP_HEIGHT));
        assert_eq!(&pixels[..4], &[10, 20, 30, 255]);
        assert_eq!(&pixels[256 * 4..256 * 4 + 4], &[40, 50, 60, 255]);
    }

    #[test]
    fn incomplete_duplicate_and_unbounded_lossless_frames_are_rejected() {
        let mut assembler = LosslessAssembler::default();
        // Desktop dimensions obey the host's limits; tiny *edge tiles* can
        // still be valid within an otherwise supported desktop.
        for (width, height) in [
            (512, 2),
            (512, 63),
            (63, 64),
            (513, 64),
            (512, 65),
            (0, 64),
            (16_384, 16_384),
        ] {
            assert!(assembler
                .push(DisplayMessage::LosslessBegin {
                    frame_id: 1,
                    base_frame_id: 0,
                    width,
                    height,
                    tile_count: 1,
                })
                .is_err());
            assert!(assembler.pending.is_none());
            assert_eq!(assembler.committed, 0);
        }
        assert!(assembler.push(begin(1, 0, 512, u32::MAX)).is_err());
        assembler.push(begin(1, 0, 512, 2)).unwrap();
        assembler.push(tile(1, 0, [1; 4])).unwrap();
        assert!(assembler.push(tile(1, 0, [2; 4])).is_err());
        assert!(assembler
            .push(DisplayMessage::LosslessCommit { frame_id: 1 })
            .is_err());
        assert_eq!(assembler.committed, 0);
        assembler.push(begin(2, 0, 512, 2)).unwrap();
        assembler.push(tile(2, 0, [3; 4])).unwrap();
        assert!(assembler
            .push(DisplayMessage::LosslessCommit { frame_id: 2 })
            .is_err());
        assert_eq!(assembler.committed, 0);
    }
}
