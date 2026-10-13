use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use ad_stream_protocol::{cache_content_key, CacheNamespace, PixelFormat};
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct CacheLimits {
    pub maximum_bytes: usize,
    pub maximum_entries: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            maximum_bytes: 256 * 1024 * 1024,
            maximum_entries: 16_384,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub data: Arc<[u8]>,
}

impl CacheEntry {
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        data: Vec<u8>,
    ) -> Result<Self, CacheError> {
        let expected = expected_size(width, height, format)?;
        if data.len() != expected {
            return Err(CacheError::InvalidPayload {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            format,
            data: data.into(),
        })
    }

    pub fn content_key(&self, namespace: CacheNamespace) -> [u8; 32] {
        cache_content_key(namespace, self.format, self.width, self.height, &self.data)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Identity {
    namespace: u8,
    key: [u8; 32],
}

impl Identity {
    fn new(namespace: CacheNamespace, key: [u8; 32]) -> Self {
        Self {
            namespace: namespace as u8,
            key,
        }
    }
}

struct StoredEntry {
    value: Arc<CacheEntry>,
    access: u64,
}

pub struct PresentationCache {
    limits: CacheLimits,
    entries: HashMap<Identity, StoredEntry>,
    access_order: BTreeMap<u64, Identity>,
    bytes: usize,
    clock: u64,
    epoch: u64,
}

impl PresentationCache {
    pub fn new(limits: CacheLimits) -> Result<Self, CacheError> {
        if limits.maximum_bytes == 0 || limits.maximum_entries == 0 {
            return Err(CacheError::InvalidLimits);
        }
        Ok(Self {
            limits,
            entries: HashMap::new(),
            access_order: BTreeMap::new(),
            bytes: 0,
            clock: 0,
            epoch: 0,
        })
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn reset(&mut self, epoch: u64) {
        self.entries.clear();
        self.access_order.clear();
        self.bytes = 0;
        self.epoch = epoch;
    }

    pub fn insert(
        &mut self,
        namespace: CacheNamespace,
        key: [u8; 32],
        entry: CacheEntry,
    ) -> Result<(), CacheError> {
        if entry.content_key(namespace) != key {
            return Err(CacheError::DigestMismatch);
        }
        if entry.data.len() > self.limits.maximum_bytes {
            return Err(CacheError::EntryTooLarge(entry.data.len()));
        }
        let identity = Identity::new(namespace, key);
        if let Some(previous) = self.entries.remove(&identity) {
            self.bytes -= previous.value.data.len();
            self.access_order.remove(&previous.access);
        }
        let value = Arc::new(entry);
        self.clock = self.clock.wrapping_add(1).max(1);
        self.bytes += value.data.len();
        self.access_order.insert(self.clock, identity);
        self.entries.insert(
            identity,
            StoredEntry {
                value,
                access: self.clock,
            },
        );
        self.evict();
        Ok(())
    }

    pub fn get(&mut self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<CacheEntry>> {
        let identity = Identity::new(namespace, *key);
        let stored = self.entries.get_mut(&identity)?;
        self.access_order.remove(&stored.access);
        self.clock = self.clock.wrapping_add(1).max(1);
        stored.access = self.clock;
        self.access_order.insert(stored.access, identity);
        Some(stored.value.clone())
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut identities: Vec<_> = self.entries.keys().copied().collect();
        identities.sort_unstable();
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(&self.epoch.to_be_bytes());
        for identity in identities {
            digest.update(&[identity.namespace]);
            digest.update(&identity.key);
        }
        digest
            .finish()
            .as_ref()
            .try_into()
            .expect("SHA-256 is 32 bytes")
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn evict(&mut self) {
        while self.bytes > self.limits.maximum_bytes
            || self.entries.len() > self.limits.maximum_entries
        {
            let Some((access, identity)) = self.access_order.pop_first() else {
                break;
            };
            if let Some(stored) = self.entries.remove(&identity) {
                debug_assert_eq!(access, stored.access);
                self.bytes -= stored.value.data.len();
            }
        }
    }
}

fn expected_size(width: u32, height: u32, format: PixelFormat) -> Result<usize, CacheError> {
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(CacheError::InvalidDimensions);
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(CacheError::SizeOverflow)?;
    match format {
        PixelFormat::Bgra8888 | PixelFormat::Rgba8888 => pixels.checked_mul(4),
        PixelFormat::Nv12 => pixels.checked_add(pixels / 2),
    }
    .ok_or(CacheError::SizeOverflow)
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum CacheError {
    #[error("cache limits must be non-zero")]
    InvalidLimits,
    #[error("cache entry dimensions are invalid")]
    InvalidDimensions,
    #[error("cache entry dimensions overflow")]
    SizeOverflow,
    #[error("cache entry contains {actual} bytes but requires {expected}")]
    InvalidPayload { expected: usize, actual: usize },
    #[error("cache entry digest does not match its ARPP key")]
    DigestMismatch,
    #[error("cache entry is too large: {0} bytes")]
    EntryTooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_content_and_evicts_lru_entries() {
        let mut cache = PresentationCache::new(CacheLimits {
            maximum_bytes: 8,
            maximum_entries: 2,
        })
        .unwrap();
        for value in [1_u8, 2, 3] {
            let entry = CacheEntry::new(1, 1, PixelFormat::Bgra8888, vec![value; 4]).unwrap();
            let key = entry.content_key(CacheNamespace::Tile);
            cache.insert(CacheNamespace::Tile, key, entry).unwrap();
        }
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.bytes(), 8);
    }

    #[test]
    fn rejects_poisoned_cache_content() {
        let mut cache = PresentationCache::new(CacheLimits::default()).unwrap();
        let entry = CacheEntry::new(1, 1, PixelFormat::Bgra8888, vec![0; 4]).unwrap();
        assert_eq!(
            cache.insert(CacheNamespace::Glyph, [9; 32], entry),
            Err(CacheError::DigestMismatch)
        );
    }
}
