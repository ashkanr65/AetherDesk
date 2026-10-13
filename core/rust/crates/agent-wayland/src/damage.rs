use std::collections::HashMap;

use ad_stream_protocol::Rectangle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceBufferKind {
    DmaBuf,
    Shm,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceSnapshot {
    pub protocol_id: u32,
    pub commits: u64,
    pub buffer_kind: SurfaceBufferKind,
}

#[derive(Clone, Debug)]
pub struct CapturedFrame {
    pub frame_id: u64,
    pub presentation_time_us: u64,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels_bgra: Vec<u8>,
    pub damage: Vec<Rectangle>,
}

#[derive(Default)]
pub(crate) struct SurfaceRegistry {
    surfaces: HashMap<u32, SurfaceSnapshot>,
}

impl SurfaceRegistry {
    pub fn committed(&mut self, protocol_id: u32, buffer_kind: Option<SurfaceBufferKind>) {
        let snapshot = self.surfaces.entry(protocol_id).or_insert(SurfaceSnapshot {
            protocol_id,
            commits: 0,
            buffer_kind: SurfaceBufferKind::Unknown,
        });
        snapshot.commits = snapshot.commits.saturating_add(1);
        if let Some(kind) = buffer_kind {
            snapshot.buffer_kind = kind;
        }
    }

    pub fn snapshots(&self) -> Vec<SurfaceSnapshot> {
        self.surfaces.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_commits_and_buffer_transitions() {
        let mut registry = SurfaceRegistry::default();
        registry.committed(7, Some(SurfaceBufferKind::Shm));
        registry.committed(7, None);
        registry.committed(7, Some(SurfaceBufferKind::DmaBuf));
        let snapshots = registry.snapshots();
        assert_eq!(snapshots[0].commits, 3);
        assert_eq!(snapshots[0].buffer_kind, SurfaceBufferKind::DmaBuf);
    }
}
