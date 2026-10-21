use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Frames may occupy the window in parallel so one round trip does not cap the
/// delivered frame rate; a byte budget keeps large bursts bounded as well.
const FRAME_WINDOW: usize = 6;
const BYTE_WINDOW: usize = 4 * 1024 * 1024;
const HISTORY_CAP: usize = FRAME_WINDOW * 4;

#[derive(Default)]
struct State {
    enabled: bool,
    last_sent: u64,
    last_ack: u64,
    pending: VecDeque<(u64, Instant, usize)>,
    pending_bytes: usize,
    baseline: Option<Duration>,
    reported_at: Option<Instant>,
}

/// Bound frames across the entire transport, including SSH and kernel buffers.
/// Legacy clients never acknowledge frames and keep the previous delivery mode.
#[derive(Default)]
pub(crate) struct DeliveryWindow {
    state: Mutex<State>,
    ready: Notify,
}

impl DeliveryWindow {
    pub async fn wait_enabled(&self) {
        loop {
            let notified = self.ready.notified();
            if self.state.lock().expect("delivery state").enabled {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait_ready(&self) {
        loop {
            let notified = self.ready.notified();
            {
                let state = self.state.lock().expect("delivery state");
                if !state.enabled
                    || (state.pending.len() < FRAME_WINDOW && state.pending_bytes < BYTE_WINDOW)
                {
                    return;
                }
            }
            notified.await;
        }
    }

    /// Lost decode dependencies cannot produce presentation receipts. Release
    /// the old window so recovery can send an independent keyframe.
    pub fn recover(&self) {
        let mut state = self.state.lock().expect("delivery state");
        state.pending.clear();
        state.pending_bytes = 0;
        state.last_ack = state.last_sent;
        self.ready.notify_one();
    }

    pub fn sent(&self, sequence: u64, bytes: usize) -> bool {
        let mut state = self.state.lock().expect("delivery state");
        state.last_sent = sequence;
        state.pending.push_back((sequence, Instant::now(), bytes));
        state.pending_bytes += bytes;
        // Bound diagnostic history even for an old client without acknowledgements.
        while state.pending.len() > HISTORY_CAP {
            if let Some((_, _, dropped)) = state.pending.pop_front() {
                state.pending_bytes = state.pending_bytes.saturating_sub(dropped);
            }
        }
        state.enabled
    }

    pub fn acknowledge(&self, sequence: u64) -> Option<Duration> {
        let mut state = self.state.lock().expect("delivery state");
        if sequence == 0 && !state.enabled {
            state.enabled = true;
            self.ready.notify_one();
            return None;
        }
        if sequence <= state.last_ack || sequence > state.last_sent {
            return None;
        }
        let position = state
            .pending
            .iter()
            .position(|(id, _, _)| *id == sequence)?;
        let elapsed = state.pending[position].1.elapsed();
        state.enabled = true;
        state.last_ack = sequence;
        let released: usize = state
            .pending
            .drain(..=position)
            .map(|(_, _, bytes)| bytes)
            .sum();
        state.pending_bytes = state.pending_bytes.saturating_sub(released);
        // Do not mistake unavoidable propagation RTT for bandwidth congestion.
        let baseline = state.baseline.map_or(elapsed, |old| old.min(elapsed));
        state.baseline = Some(baseline);
        if state
            .reported_at
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(5))
        {
            tracing::info!(
                receipt_ms = elapsed.as_millis() as u64,
                excess_ms = elapsed.saturating_sub(baseline).as_millis() as u64,
                queued_frames = state.pending.len(),
                queued_bytes = state.pending_bytes,
                "remote display receipt performance"
            );
            state.reported_at = Some(Instant::now());
        }
        self.ready.notify_one();
        Some(elapsed.saturating_sub(baseline))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_releases_unpresentable_frames() {
        let window = DeliveryWindow::default();
        window.acknowledge(0);
        for sequence in 1..=FRAME_WINDOW as u64 {
            window.sent(sequence, 1024);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(10), window.wait_ready())
                .await
                .is_err()
        );
        window.recover();
        tokio::time::timeout(Duration::from_millis(100), window.wait_ready())
            .await
            .unwrap();
        assert!(window.acknowledge(1).is_none());
        window.sent(FRAME_WINDOW as u64 + 1, 1024);
        assert!(window.acknowledge(FRAME_WINDOW as u64 + 1).is_some());
    }

    #[tokio::test]
    async fn window_in_flight_and_cumulative_acknowledgements() {
        let window = DeliveryWindow::default();
        window.acknowledge(0);
        for sequence in 10..10 + FRAME_WINDOW as u64 {
            assert!(window.sent(sequence, 100));
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
                .await
                .is_err()
        );
        assert!(window.acknowledge(100).is_none());
        assert!(window.acknowledge(9).is_none());
        assert!(window.acknowledge(10 + FRAME_WINDOW as u64 - 1).is_some());
        tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
            .await
            .unwrap();
        assert!(window.state.lock().unwrap().pending.is_empty());
        assert_eq!(window.state.lock().unwrap().pending_bytes, 0);
        assert!(window.acknowledge(10 + FRAME_WINDOW as u64 - 1).is_none());
    }

    #[tokio::test]
    async fn a_byte_budget_bounds_large_bursts_independently_of_the_frame_window() {
        let window = DeliveryWindow::default();
        window.acknowledge(0);
        assert!(window.sent(1, BYTE_WINDOW));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
                .await
                .is_err()
        );
        assert!(window.acknowledge(1).is_some());
        tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn old_clients_are_not_blocked_and_history_is_bounded() {
        let window = DeliveryWindow::default();
        for sequence in 1..100 {
            tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
                .await
                .unwrap();
            assert!(!window.sent(sequence, 64));
        }
        assert_eq!(window.state.lock().unwrap().pending.len(), HISTORY_CAP);
    }

    #[tokio::test]
    async fn client_receipts_release_the_host_window_over_the_wire() {
        use ad_stream_client::{ArppSession, SshChannelIo, SshTransportAdapter, StreamDemuxer};
        use ad_stream_protocol::{ArppMessage, ControlMessage, DisplayMessage, VideoCodec};
        use std::sync::Arc;
        let (control, peer_control) = tokio::io::duplex(4096);
        let (input, peer_input) = tokio::io::duplex(4096);
        let (media, peer_media) = tokio::io::duplex(4096);
        let host = ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        )));
        let client = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(peer_control),
            SshChannelIo::from_stream(peer_input),
            SshChannelIo::from_stream(peer_media),
        ))));
        let (mut demuxer, streams) = StreamDemuxer::spawn(client);
        let window = DeliveryWindow::default();
        let ready = tokio::time::timeout(Duration::from_secs(1), host.receive())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ready.message,
            ArppMessage::Control(ControlMessage::Acknowledge { sequence: 0 })
        );
        window.acknowledge(0);
        let last = FRAME_WINDOW as u64;
        for frame_id in 1..=last {
            assert!(window.sent(frame_id, 100));
            host.send(ArppMessage::Display(DisplayMessage::EncodedFrame {
                frame_id,
                presentation_time_us: frame_id,
                codec: VideoCodec::H264,
                key_frame: true,
                width: 64,
                height: 64,
                data: vec![0; 100],
            }))
            .await
            .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), window.wait_ready())
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            // Presentation receipts (not network receipts) release the window.
            for frame_id in 1..=last {
                streams.receipts.send_replace(frame_id);
            }
            loop {
                if let ArppMessage::Control(ControlMessage::Acknowledge { sequence }) =
                    host.receive().await.unwrap().message
                {
                    if window.acknowledge(sequence).is_some() && sequence == last {
                        break;
                    }
                }
            }
            window.wait_ready().await;
        })
        .await
        .unwrap();
        assert!(window.state.lock().unwrap().pending.is_empty());
        demuxer.shutdown_and_wait().await;
    }
}
