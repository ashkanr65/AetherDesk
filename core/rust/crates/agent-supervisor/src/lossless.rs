//! Latest complete capture -> independent changed tiles -> atomic presentation.
//! Compare against the last SENT complete image, never just the newest capture's
//! damage: the watch channel is allowed to skip intermediate captures.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ad_stream_client::{ArppSession, TransportError};
use ad_stream_protocol::{
    lossless::{LosslessTile, TILE_EDGE},
    ArppMessage, DisplayMessage, Rectangle,
};
use agent_wayland::damage::CapturedFrame;
use tokio::sync::watch;

use crate::{delivery::DeliveryWindow, server::AgentServerError};

pub(crate) struct Source {
    pub frames: watch::Receiver<Option<Arc<CapturedFrame>>>,
    pub monitors: watch::Receiver<Vec<Rectangle>>,
    pub rate: watch::Receiver<(u32, u32)>,
    pub refresh: watch::Receiver<u64>,
}

struct Batch {
    tiles: Vec<LosslessTile>,
    bytes: usize,
}

fn overlaps(a: Rectangle, b: Rectangle) -> bool {
    u64::from(a.x) < u64::from(b.x) + u64::from(b.width)
        && u64::from(b.x) < u64::from(a.x) + u64::from(a.width)
        && u64::from(a.y) < u64::from(b.y) + u64::from(b.height)
        && u64::from(b.y) < u64::from(a.y) + u64::from(a.height)
}

fn build(
    frame: &CapturedFrame,
    previous: Option<&CapturedFrame>,
    monitors: &[Rectangle],
) -> Result<Batch, &'static str> {
    if !ad_stream_protocol::valid_monitor_layout(frame.width, frame.height, monitors)
        || frame.stride < frame.width as usize * 4
        || frame
            .stride
            .checked_mul(frame.height as usize)
            .is_none_or(|n| n > frame.pixels_bgra.len())
    {
        return Err("Invalid lossless capture");
    }
    let previous = previous.filter(|p| {
        p.width == frame.width
            && p.height == frame.height
            && p.stride >= p.width as usize * 4
            && p.stride
                .checked_mul(p.height as usize)
                .is_some_and(|n| n <= p.pixels_bgra.len())
    });
    let mut tiles = Vec::new();
    let mut bytes = 0;
    for y in (0..frame.height).step_by(TILE_EDGE as usize) {
        for x in (0..frame.width).step_by(TILE_EDGE as usize) {
            let region = Rectangle {
                x,
                y,
                width: TILE_EDGE.min(frame.width - x),
                height: TILE_EDGE.min(frame.height - y),
            };
            // A global grid deduplicates overlapping monitors and bounds every
            // patch. Tiles wholly in unused canvas gaps never cross the wire.
            if !monitors.iter().any(|monitor| overlaps(region, *monitor)) {
                continue;
            }
            let row_bytes = region.width as usize * 4;
            if previous.is_some_and(|p| {
                (0..region.height as usize).all(|row| {
                    let a = (y as usize + row) * frame.stride + x as usize * 4;
                    let b = (y as usize + row) * p.stride + x as usize * 4;
                    frame.pixels_bgra[a..a + row_bytes] == p.pixels_bgra[b..b + row_bytes]
                })
            }) {
                continue;
            }
            let mut pixels = Vec::with_capacity(row_bytes * region.height as usize);
            for row in 0..region.height as usize {
                let start = (y as usize + row) * frame.stride + x as usize * 4;
                pixels.extend_from_slice(&frame.pixels_bgra[start..start + row_bytes]);
            }
            let tile = LosslessTile::encode(region, pixels)?;
            bytes += tile.data.len();
            tiles.push(tile);
        }
    }
    Ok(Batch { tiles, bytes })
}

fn protocol_error(detail: &str) -> AgentServerError {
    TransportError::Connection(detail.into()).into()
}

pub(crate) async fn serve(
    mut source: Source,
    session: &Arc<ArppSession>,
    delivery: &DeliveryWindow,
) -> Result<(), AgentServerError> {
    // The receipt opt-in is emitted by the running display consumer, AFTER the
    // Start/Resume handshake. Never race display data against that handshake.
    tokio::time::timeout(Duration::from_secs(15), delivery.wait_enabled())
        .await
        .map_err(|_| protocol_error("Lossless presentation consumer did not start"))?;
    tracing::info!(
        tile_edge = TILE_EDGE,
        "remote display uses exact BGRA lossless tiles"
    );
    let mut force_full = true;
    let mut previous: Option<Arc<CapturedFrame>> = None;
    let mut sequence = 0_u64;
    let mut next_frame = tokio::time::Instant::now();
    let mut report_at = Instant::now();
    let (mut report_frames, mut report_tiles, mut report_bytes, mut preparation_us, mut write_us) =
        (0_u64, 0_u64, 0_u64, 0_u128, 0_u128);
    loop {
        if !force_full
            && !source
                .frames
                .has_changed()
                .map_err(|_| protocol_error("Capture stopped"))?
        {
            tokio::select! {
                changed = source.frames.changed() => changed.map_err(|_| protocol_error("Capture stopped"))?,
                changed = source.monitors.changed() => { changed.map_err(|_| protocol_error("Monitor layout stopped"))?; force_full = true; },
                changed = source.refresh.changed() => {
                    changed.map_err(|_| protocol_error("Refresh controller stopped"))?;
                    force_full = true;
                },
            }
        }
        if source.monitors.has_changed().unwrap_or(false)
            || source.refresh.has_changed().unwrap_or(false)
        {
            force_full = true;
        }
        source.refresh.borrow_and_update();
        let monitors = source.monitors.borrow_and_update().clone();
        tokio::time::timeout(Duration::from_secs(15), delivery.wait_ready())
            .await
            .map_err(|_| protocol_error("Lossless presentation acknowledgement timed out"))?;
        tokio::time::sleep_until(next_frame).await;
        // Fixed upper cadence, with congestion controlling frequency only. Exact
        // pixel values never change when delivery slows down.
        let fps = source.rate.borrow().1.clamp(1, 240);
        next_frame = tokio::time::Instant::now() + Duration::from_secs_f64(1.0 / fps as f64);
        let frame = source.frames.borrow_and_update().clone();
        let Some(frame) = frame else {
            source
                .frames
                .changed()
                .await
                .map_err(|_| protocol_error("Capture stopped"))?;
            continue;
        };
        if !ad_stream_protocol::valid_monitor_layout(frame.width, frame.height, &monitors) {
            // Capture may still be at the previous size while RandR settles.
            source
                .frames
                .changed()
                .await
                .map_err(|_| protocol_error("Capture stopped"))?;
            force_full = true;
            continue;
        }
        force_full |= previous
            .as_ref()
            .is_none_or(|p| p.width != frame.width || p.height != frame.height);
        let base = if force_full { None } else { previous.clone() };
        let started = Instant::now();
        let capture = frame.clone();
        // One compression job per session, away from input/control workers.
        let batch =
            tokio::task::spawn_blocking(move || build(&capture, base.as_deref(), &monitors))
                .await
                .map_err(|_| protocol_error("Lossless preparation worker stopped"))?
                .map_err(protocol_error)?;
        preparation_us += started.elapsed().as_micros();
        if batch.tiles.is_empty() {
            previous = Some(frame);
            continue;
        }
        let base_frame_id = if force_full { 0 } else { sequence };
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| protocol_error("Display sequence exhausted"))?;
        let tile_count = batch.tiles.len();
        let bytes = batch.bytes;
        let started = Instant::now();
        // Charge the whole transaction before sending it. Begin/tiles/commit
        // stay on the same ordered media stream; only COMMIT earns a receipt.
        delivery.sent(sequence, bytes + tile_count * 64 + 128);
        session
            .send(ArppMessage::Display(DisplayMessage::LosslessBegin {
                frame_id: sequence,
                base_frame_id,
                width: frame.width,
                height: frame.height,
                tile_count: tile_count as u32,
            }))
            .await?;
        for tile in batch.tiles {
            session
                .send(ArppMessage::Display(DisplayMessage::LosslessTile {
                    frame_id: sequence,
                    tile,
                }))
                .await?;
        }
        session
            .send(ArppMessage::Display(DisplayMessage::LosslessCommit {
                frame_id: sequence,
            }))
            .await?;
        write_us += started.elapsed().as_micros();
        previous = Some(frame);
        force_full = false;
        report_frames += 1;
        report_tiles += tile_count as u64;
        report_bytes += bytes as u64;
        if report_at.elapsed() >= Duration::from_secs(5) {
            tracing::info!(
                interval_ms = report_at.elapsed().as_millis() as u64,
                frames = report_frames,
                tiles = report_tiles,
                payload_bytes = report_bytes,
                preparation_us = preparation_us as u64,
                write_us = write_us as u64,
                "remote lossless display performance"
            );
            report_at = Instant::now();
            (
                report_frames,
                report_tiles,
                report_bytes,
                preparation_us,
                write_us,
            ) = (0, 0, 0, 0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(width: u32, height: u32) -> CapturedFrame {
        CapturedFrame {
            frame_id: 1,
            presentation_time_us: 0,
            width,
            height,
            stride: width as usize * 4,
            pixels_bgra: [7, 21, 240, 255].repeat(width as usize * height as usize),
            damage: vec![],
        }
    }

    #[test]
    fn changed_tiles_compare_the_sent_base_and_leave_idle_monitors_alone() {
        let first = frame(1024, 512);
        let monitors = [
            Rectangle {
                x: 0,
                y: 0,
                width: 512,
                height: 512,
            },
            Rectangle {
                x: 512,
                y: 0,
                width: 512,
                height: 512,
            },
        ];
        let initial = build(&first, None, &monitors).unwrap();
        assert_eq!(initial.tiles.len(), 8);
        assert!(initial.bytes < first.pixels_bgra.len() / 10);
        let mut skipped = first.clone();
        skipped.pixels_bgra[0..4].copy_from_slice(&[1, 2, 3, 255]);
        let mut newest = skipped.clone();
        newest.pixels_bgra[256 * 4..256 * 4 + 4].copy_from_slice(&[4, 5, 6, 255]);
        // The latest damage intentionally omits the earlier change.
        newest.damage = vec![Rectangle {
            x: 256,
            y: 0,
            width: 1,
            height: 1,
        }];
        let delta = build(&newest, Some(&first), &monitors).unwrap();
        assert_eq!(delta.tiles.len(), 2);
        assert!(delta.tiles.iter().all(|t| t.region.x < 512));
        assert_eq!(
            &delta.tiles[0].clone().decode().unwrap()[..4],
            &[1, 2, 3, 255]
        );
        assert!(build(&newest, Some(&newest), &monitors)
            .unwrap()
            .tiles
            .is_empty());
        assert_eq!(
            build(&newest, None, &monitors).unwrap().tiles.len(),
            8,
            "refresh must rebuild every visible tile"
        );
    }

    #[test]
    fn lossless_monitor_gaps_overlaps_padded_rows_and_large_canvases_are_bounded() {
        let large = frame(4800, 1800);
        let monitors = [
            Rectangle {
                x: 0,
                y: 512,
                width: 1920,
                height: 1024,
            },
            Rectangle {
                x: 1920,
                y: 0,
                width: 2880,
                height: 1800,
            },
        ];
        let batch = build(&large, None, &monitors).unwrap();
        assert!(!batch.tiles.is_empty());
        assert!(batch
            .tiles
            .iter()
            .all(|t| t.region.x >= 1792 || (t.region.y >= 512 && t.region.y < 1536)));
        assert!(batch.tiles.iter().all(|t| t.data.len() <= 256 * 256 * 4));
        let small = frame(256, 2);
        let overlapping = vec![
            Rectangle {
                x: 0,
                y: 0,
                width: 256,
                height: 2
            };
            16
        ];
        assert_eq!(build(&small, None, &overlapping).unwrap().tiles.len(), 1);
        let mut padded = small.clone();
        padded.stride += 16;
        padded.pixels_bgra = small.pixels_bgra[..1024]
            .iter()
            .copied()
            .chain([19; 16])
            .chain(small.pixels_bgra[1024..].iter().copied())
            .chain([19; 16])
            .collect();
        assert_eq!(
            build(&padded, None, &overlapping).unwrap().tiles[0]
                .clone()
                .decode()
                .unwrap(),
            small.pixels_bgra
        );
    }

    #[tokio::test]
    async fn lossless_wire_commits_receipts_refresh_and_input_survive_backpressure() {
        use ad_display_decoder::{Framebuffer, LosslessAssembler};
        use ad_stream_client::{SshChannelIo, SshTransportAdapter, StreamDemuxer};
        use ad_stream_protocol::{ControlMessage, InputMessage};
        use tokio::io::duplex;
        let (a0, b0) = duplex(1024);
        let (a1, b1) = duplex(1024);
        let (a2, b2) = duplex(1024);
        let host = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(a0),
            SshChannelIo::from_stream(a1),
            SshChannelIo::from_stream(a2),
        ))));
        let client = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(b0),
            SshChannelIo::from_stream(b1),
            SshChannelIo::from_stream(b2),
        ))));
        let (demuxer, mut channels) = StreamDemuxer::spawn(client.clone());
        let delivery = Arc::new(DeliveryWindow::default());
        let first = Arc::new(frame(1024, 512));
        let (frames, receiver) = watch::channel(Some(first.clone()));
        let (_monitors, monitors) = watch::channel(vec![Rectangle {
            x: 0,
            y: 0,
            width: 1024,
            height: 512,
        }]);
        let (_rate, rate) = watch::channel((8000, 30));
        let (refresh, refresh_rx) = watch::channel(0);
        let (input_seen, input_received) = tokio::sync::oneshot::channel();
        let host_read = host.clone();
        let receipts = delivery.clone();
        let input_reader = tokio::spawn(async move {
            let mut input_seen = Some(input_seen);
            loop {
                match host_read.receive().await.unwrap().message {
                    ArppMessage::Control(ControlMessage::Acknowledge { sequence }) => {
                        receipts.acknowledge(sequence);
                    }
                    ArppMessage::Input(InputMessage::PointerMove { x: 99, .. }) => {
                        if let Some(reply) = input_seen.take() {
                            let _ = reply.send(());
                        }
                    }
                    _ => {}
                }
            }
        });
        let host_write = host.clone();
        let credits = delivery.clone();
        let writer = tokio::spawn(async move {
            serve(
                Source {
                    frames: receiver,
                    monitors,
                    rate,
                    refresh: refresh_rx,
                },
                &host_write,
                &credits,
            )
            .await
        });
        // Saturate the eight-message display queue (begin + eight tiles + commit).
        // The separate input path must still make progress while it is blocked.
        tokio::time::timeout(Duration::from_secs(5), async {
            while channels.display.len() < 8 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client
            .send(ArppMessage::Input(InputMessage::PointerMove {
                x: 99,
                y: 10,
                monitor: 0,
            }))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), input_received)
            .await
            .unwrap()
            .unwrap();
        let mut assembler = LosslessAssembler::default();
        async fn read_commit(
            channels: &mut ad_stream_client::DemuxedStreams,
            assembler: &mut LosslessAssembler,
        ) -> ad_display_decoder::LosslessUpdate {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let message = channels.display.recv().await.unwrap();
                    if let Some(update) = assembler.push(message).unwrap() {
                        return update;
                    }
                }
            })
            .await
            .unwrap()
        }
        let update = read_commit(&mut channels, &mut assembler).await;
        assert_eq!(update.base_frame_id, 0);
        assert_eq!(update.patches.len(), 8);
        channels.receipts.send_replace(update.frame_id);
        let mut buffer = Framebuffer::new(1024, 512).unwrap();
        buffer.apply_lossless(update).unwrap();
        assert_eq!(&*buffer.snapshot().2, &first.pixels_bgra);
        let mut next = (*first).clone();
        next.pixels_bgra[..4].copy_from_slice(&[12, 34, 56, 255]);
        frames.send_replace(Some(Arc::new(next)));
        let update = read_commit(&mut channels, &mut assembler).await;
        assert_eq!(update.base_frame_id, 1);
        assert_eq!(update.patches.len(), 1);
        channels.receipts.send_replace(update.frame_id);
        buffer.apply_lossless(update).unwrap();
        assert_eq!(&buffer.snapshot().2[..4], &[12, 34, 56, 255]);
        refresh.send_replace(1);
        let update = read_commit(&mut channels, &mut assembler).await;
        assert_eq!(
            update.base_frame_id, 0,
            "recovery must not depend on old tiles"
        );
        assert_eq!(update.patches.len(), 8);
        writer.abort();
        input_reader.abort();
        demuxer.shutdown();
        let _ = writer.await;
        let _ = input_reader.await;
    }
}
