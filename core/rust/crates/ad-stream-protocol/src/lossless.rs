//! Opt-in, bounded, exact BGRA tiles. No colour conversion or chroma subsampling.
//!
//! The hello nonce prefix advertises this extension without changing any v3
//! message layout. Other opt-ins reserve their own marker bytes; the unreserved
//! suffix stays random. Nonces are NOT channel
//! authentication: the separate random 256-bit channel token still is.
//! An old peer ignores the prefix and uses the existing video messages. Only a
//! client that understands these appended display variants may set the prefix.

use serde::{Deserialize, Serialize};

use crate::Rectangle;

pub const HELLO_PREFIX: &[u8; 8] = b"ADLT0001";
pub const TILE_EDGE: u32 = 256;
pub const MAX_TILES: usize = 8192;
pub const MAX_FRAME_BYTES: usize = 33_554_432 * 4;

pub fn advertise(nonce: &mut [u8; 32]) {
    nonce[..HELLO_PREFIX.len()].copy_from_slice(HELLO_PREFIX);
}

pub fn supported(nonce: &[u8; 32]) -> bool {
    nonce.starts_with(HELLO_PREFIX)
}

/// Independent optional capability. Old agents still see the original lossless
/// and cursor markers and ignore this one. The last 64 nonce bits stay random;
/// channel authentication continues to use its separate random 256-bit token.
pub fn advertise_tile_reuse(nonce: &mut [u8; 32]) {
    nonce[16..24].copy_from_slice(b"ADTC0001");
}
pub fn tile_reuse_supported(nonce: &[u8; 32]) -> bool {
    supported(nonce) && &nonce[16..24] == b"ADTC0001"
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TileEncoding {
    Bgra,
    Lz4Bgra,
    /// Copy a complete same-sized tile from the last COMMITTED baseline,
    /// never from an earlier tile in this transaction. Data must be empty.
    CopyBgra {
        x: u32,
        y: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LosslessTile {
    pub region: Rectangle,
    pub encoding: TileEncoding,
    pub data: Vec<u8>,
}

impl LosslessTile {
    pub fn encode(region: Rectangle, bgra: Vec<u8>) -> Result<Self, &'static str> {
        if tile_bytes(region)? != bgra.len() {
            return Err("Invalid lossless tile storage");
        }
        let compressed = lz4_flex::block::compress(&bgra);
        let (encoding, data) = if compressed.len() < bgra.len() {
            (TileEncoding::Lz4Bgra, compressed)
        } else {
            (TileEncoding::Bgra, bgra)
        };
        Ok(Self {
            region,
            encoding,
            data,
        })
    }

    pub fn decode(self) -> Result<Vec<u8>, &'static str> {
        let expected = tile_bytes(self.region)?;
        // Never allocate based on an untrusted embedded decompressed size.
        if self.data.len() > expected {
            return Err("Oversized lossless tile");
        }
        match self.encoding {
            TileEncoding::Bgra if self.data.len() == expected => Ok(self.data),
            TileEncoding::Bgra => Err("Incomplete lossless tile"),
            TileEncoding::Lz4Bgra => {
                let mut output = vec![0; expected];
                let count = lz4_flex::block::decompress_into(&self.data, &mut output)
                    .map_err(|_| "Invalid compressed lossless tile")?;
                if count != expected {
                    return Err("Incomplete compressed lossless tile");
                }
                Ok(output)
            }
            TileEncoding::CopyBgra { .. } => Err("Copied tiles require a committed baseline"),
        }
    }
}

pub fn tile_bytes(region: Rectangle) -> Result<usize, &'static str> {
    if region.width == 0
        || region.height == 0
        || region.width > TILE_EDGE
        || region.height > TILE_EDGE
    {
        return Err("Invalid lossless tile dimensions");
    }
    Ok(region.width as usize * region.height as usize * 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArppFrame, ArppMessage, DisplayMessage};

    #[test]
    fn tile_reuse_negotiation_keeps_legacy_lossless_cursor_and_random_suffix() {
        let mut nonce = [91; 32];
        advertise(&mut nonce);
        crate::advertise_cursor_shapes(&mut nonce);
        assert!(!tile_reuse_supported(&nonce));
        advertise_tile_reuse(&mut nonce);
        assert!(supported(&nonce));
        assert!(crate::cursor_shapes_supported(&nonce));
        assert!(tile_reuse_supported(&nonce));
        assert_eq!(&nonce[24..], &[91; 8]);
        let tile = LosslessTile {
            region: Rectangle {
                x: 0,
                y: 0,
                width: 256,
                height: 64,
            },
            encoding: TileEncoding::CopyBgra { x: 256, y: 0 },
            data: Vec::new(),
        };
        assert!(tile.clone().decode().is_err());
        let message = ArppFrame::new(
            2,
            ArppMessage::Display(DisplayMessage::LosslessTile { frame_id: 3, tile }),
        );
        let mut wire = bytes::BytesMut::from(&crate::encode_frame(&message).unwrap()[..]);
        assert_eq!(crate::decode_frame(&mut wire).unwrap().unwrap(), message);
        nonce[0] ^= 1;
        assert!(!tile_reuse_supported(&nonce));
    }

    #[test]
    fn lossless_tiles_preserve_every_channel_and_bound_decompression() {
        let region = Rectangle {
            x: 0,
            y: 0,
            width: 256,
            height: 256,
        };
        let pixels: Vec<u8> = (0..256 * 256 * 4).map(|i| (i % 251) as u8).collect();
        let tile = LosslessTile::encode(region, pixels.clone()).unwrap();
        assert!(tile.data.len() < pixels.len());
        assert_eq!(tile.clone().decode().unwrap(), pixels);
        let message = ArppFrame::new(
            1,
            ArppMessage::Display(DisplayMessage::LosslessTile { frame_id: 1, tile }),
        );
        let mut bytes = bytes::BytesMut::from(&crate::encode_frame(&message).unwrap()[..]);
        assert_eq!(crate::decode_frame(&mut bytes).unwrap().unwrap(), message);
        for data in [vec![], vec![0; 256 * 256 * 4 + 1], vec![255; 17]] {
            assert!(LosslessTile {
                region,
                encoding: TileEncoding::Lz4Bgra,
                data
            }
            .decode()
            .is_err());
        }
        assert!(tile_bytes(Rectangle {
            width: u32::MAX,
            ..region
        })
        .is_err());
        let mut nonce = [19; 32];
        assert!(!supported(&nonce));
        advertise(&mut nonce);
        assert!(supported(&nonce));
        assert_eq!(&nonce[8..], &[19; 24]);
    }
}
