//! Opt-in, bounded, exact BGRA tiles. No colour conversion or chroma subsampling.
//!
//! The hello nonce prefix advertises this extension without changing any v3
//! message layout. The remaining 192 bits stay random. Nonces are NOT channel
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TileEncoding {
    Bgra,
    Lz4Bgra,
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
