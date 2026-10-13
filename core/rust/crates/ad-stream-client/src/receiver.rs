use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ad_stream_protocol::{
    ArppMessage, AudioMessage, BulkMessage, ClipboardMessage, ControlMessage, DisplayMessage,
    InputMessage, MetricsMessage,
};
use thiserror::Error;
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::JoinHandle;

use crate::{ArppSession, TransportError};

pub struct DemuxedStreams {
    pub input: mpsc::Sender<InputMessage>,
    pub control: mpsc::Receiver<ControlMessage>,
    pub clipboard: mpsc::Receiver<ClipboardMessage>,
    pub display: mpsc::Receiver<DisplayMessage>,
    pub audio: mpsc::Receiver<AudioMessage>,
    pub metrics: mpsc::Receiver<MetricsMessage>,
    pub bulk: mpsc::Receiver<BulkMessage>,
    pub errors: watch::Receiver<Option<String>>,
    /// Presentation receipts: consumers publish the frame id of each frame
    /// they actually decoded and applied. The host window must reflect the
    /// real render backlog, not just network receipt.
    pub receipts: watch::Sender<u64>,
}

/// Measured client load reported back to the host instead of fixed values.
#[derive(Default)]
struct Feedback {
    dropped: AtomicU64,
}

pub struct StreamDemuxer {
    receive_task: JoinHandle<()>,
    input_task: JoinHandle<()>,
    feedback_task: JoinHandle<()>,
    receipt_task: JoinHandle<()>,
}

impl StreamDemuxer {
    pub fn spawn(session: Arc<ArppSession>) -> (Self, DemuxedStreams) {
        let (input_tx, mut input_rx) = mpsc::channel::<InputMessage>(1024);
        let (control_tx, control) = mpsc::channel(64);
        let (clipboard_tx, clipboard) = mpsc::channel(32);
        let (display_tx, display) = mpsc::channel(8);
        let (audio_tx, audio) = mpsc::channel(32);
        let (metrics_tx, metrics) = mpsc::channel(32);
        let (bulk_tx, bulk) = mpsc::channel(8);
        let (error_tx, errors) = watch::channel(None);
        let (receipt_tx, mut receipt_rx) = watch::channel(0_u64);
        let feedback = Arc::new(Feedback::default());
        let receipt_session = session.clone();
        let receipt_errors = error_tx.clone();
        let receipt_task = tokio::spawn(async move {
            // This existing control message is ignored by old agents. Zero
            // opts in; later values acknowledge decoded and applied video frames.
            loop {
                let sequence = *receipt_rx.borrow_and_update();
                if let Err(error) = receipt_session
                    .send(ArppMessage::Control(ControlMessage::Acknowledge {
                        sequence,
                    }))
                    .await
                {
                    receipt_errors.send_replace(Some(error.to_string()));
                    return;
                }
                if receipt_rx.changed().await.is_err() {
                    return;
                }
            }
        });

        let input_session = session.clone();
        let input_errors = error_tx.clone();
        let input_task = tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                if let Err(error) = input_session.send(ArppMessage::Input(input)).await {
                    input_errors.send_replace(Some(error.to_string()));
                    break;
                }
            }
        });

        let refresh = Arc::new(Notify::new());
        let feedback_refresh = refresh.clone();
        let feedback_session = session.clone();
        let feedback_errors = error_tx.clone();
        let feedback_load = feedback.clone();
        let feedback_queue = display_tx.clone();
        let presented = receipt_tx.subscribe();
        let feedback_task = tokio::spawn(async move {
            loop {
                feedback_refresh.notified().await;
                // Report measured load; fixed values once made the host cut
                // quality on every report instead of real overload.
                let dropped_frames = feedback_load.dropped.swap(0, Ordering::Relaxed);
                let queue_depth = feedback_queue.max_capacity() - feedback_queue.capacity();
                let last_decoded_sequence = *presented.borrow();
                for message in [
                    ArppMessage::Metrics(MetricsMessage::DecoderLoad {
                        queue_depth: queue_depth.min(usize::from(u16::MAX)) as u16,
                        decode_time_us: 0,
                        dropped_frames: dropped_frames.min(u64::from(u32::MAX)) as u32,
                    }),
                    ArppMessage::Control(ControlMessage::RequestKeyFrame {
                        last_decoded_sequence,
                    }),
                ] {
                    if let Err(error) = feedback_session.send(message).await {
                        feedback_errors.send_replace(Some(error.to_string()));
                        return;
                    }
                }
                // Coalesce overload reports instead of flooding the host with IDRs.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });
        let receive_task = tokio::spawn(async move {
            let mut recovering = false;
            loop {
                let frame = match session.receive().await {
                    Ok(frame) => frame,
                    Err(error) => {
                        error_tx.send_replace(Some(error.to_string()));
                        break;
                    }
                };
                let result = match frame.message {
                    ArppMessage::Control(message) => {
                        enqueue(&control_tx, message, "control", false)
                    }
                    ArppMessage::Clipboard(message) => {
                        enqueue(&clipboard_tx, message, "clipboard", false)
                    }
                    ArppMessage::Display(message) => {
                        queue_display(&display_tx, message, &mut recovering, &feedback)
                            .await
                            .map(|request| {
                                if request {
                                    refresh.notify_one();
                                }
                            })
                    }
                    ArppMessage::Audio(message) => enqueue(&audio_tx, message, "audio", true),
                    ArppMessage::Metrics(message) => enqueue(&metrics_tx, message, "metrics", true),
                    ArppMessage::Bulk(message) => enqueue(&bulk_tx, message, "bulk", false),
                    ArppMessage::Input(_) => {
                        error_tx.send_replace(Some(
                            "server sent an input-class message to the client".into(),
                        ));
                        break;
                    }
                };
                if let Err(queue) = result {
                    error_tx.send_replace(Some(format!("{queue} consumer closed")));
                    break;
                }
            }
        });

        (
            Self {
                receive_task,
                input_task,
                feedback_task,
                receipt_task,
            },
            DemuxedStreams {
                input: input_tx,
                control,
                clipboard,
                display,
                audio,
                metrics,
                bulk,
                errors,
                receipts: receipt_tx,
            },
        )
    }

    pub fn is_finished(&self) -> bool {
        self.receive_task.is_finished()
            || self.input_task.is_finished()
            || self.feedback_task.is_finished()
            || self.receipt_task.is_finished()
    }

    pub fn shutdown(&self) {
        self.receive_task.abort();
        self.input_task.abort();
        self.feedback_task.abort();
        self.receipt_task.abort();
    }

    /// Return ownership of session.receive only after all workers have stopped.
    pub async fn shutdown_and_wait(&mut self) {
        self.shutdown();
        let _ = (&mut self.receive_task).await;
        let _ = (&mut self.input_task).await;
        let _ = (&mut self.feedback_task).await;
        let _ = (&mut self.receipt_task).await;
    }
}

impl Drop for StreamDemuxer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn queue_display(
    sender: &mpsc::Sender<DisplayMessage>,
    message: DisplayMessage,
    recovering: &mut bool,
    feedback: &Feedback,
) -> Result<bool, &'static str> {
    if let DisplayMessage::EncodedFrame { key_frame, .. } = &message {
        let key_frame = *key_frame;
        if *recovering && !key_frame {
            feedback.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(false);
        }
        match sender.try_send(message) {
            Ok(()) => {
                if key_frame {
                    *recovering = false;
                }
                Ok(false)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                feedback.dropped.fetch_add(1, Ordering::Relaxed);
                *recovering = true;
                Ok(true)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err("display"),
        }
    } else {
        // Cache mutations and patches must never be silently discarded.
        sender
            .send(message)
            .await
            .map(|_| false)
            .map_err(|_| "display")
    }
}

fn enqueue<T>(
    sender: &mpsc::Sender<T>,
    message: T,
    name: &'static str,
    lossy_when_full: bool,
) -> Result<(), &'static str> {
    match sender.try_send(message) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) if lossy_when_full => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(name)
        }
    }
}

#[derive(Debug, Error)]
pub enum StreamReceiverError {
    #[error("ARPP transport failed: {0}")]
    Transport(#[from] TransportError),
    #[error("ARPP input queue is closed")]
    InputClosed,
}

impl DemuxedStreams {
    pub async fn send_input(&self, input: InputMessage) -> Result<(), StreamReceiverError> {
        self.input
            .send(input)
            .await
            .map_err(|_| StreamReceiverError::InputClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ad_stream_protocol::VideoCodec;

    #[tokio::test]
    async fn full_video_queue_does_not_block_control_or_keyframe_feedback() {
        use crate::{SshChannelIo, SshTransportAdapter};
        use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
        async fn read_peer(peer: &mut tokio::io::DuplexStream) -> ad_stream_protocol::ArppFrame {
            let length = peer.read_u32().await.unwrap();
            let mut bytes = bytes::BytesMut::from(length.to_be_bytes().as_slice());
            bytes.resize(length as usize + 4, 0);
            peer.read_exact(&mut bytes[4..]).await.unwrap();
            ad_stream_protocol::decode_frame(&mut bytes)
                .unwrap()
                .unwrap()
        }
        let (control, mut control_peer) = duplex(16384);
        let (input, _input_peer) = duplex(1024);
        let (media, mut media_peer) = duplex(16384);
        let session = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        ))));
        let (_demuxer, mut channels) = StreamDemuxer::spawn(session);
        for sequence in 0..16 {
            let message = DisplayMessage::EncodedFrame {
                frame_id: sequence,
                presentation_time_us: sequence,
                codec: VideoCodec::H264,
                key_frame: sequence == 0,
                width: 64,
                height: 64,
                data: vec![0; 100],
            };
            media_peer
                .write_all(
                    &ad_stream_protocol::encode_frame(&ad_stream_protocol::ArppFrame::new(
                        sequence,
                        ArppMessage::Display(message),
                    ))
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        // Presentation receipts are published by the frame consumer.
        channels.receipts.send_replace(15);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let (mut report, mut refresh, mut receipt) = (false, false, false);
            while !report || !refresh || !receipt {
                match read_peer(&mut control_peer).await.message {
                    ArppMessage::Metrics(MetricsMessage::DecoderLoad {
                        dropped_frames, ..
                    }) if dropped_frames > 0 => report = true,
                    // Idle interval reports carry no drops or blocked frames.
                    ArppMessage::Metrics(MetricsMessage::DecoderLoad { .. }) => {}
                    ArppMessage::Control(ControlMessage::RequestKeyFrame { .. }) => refresh = true,
                    ArppMessage::Control(ControlMessage::Acknowledge { sequence: 15 }) => {
                        receipt = true
                    }
                    ArppMessage::Control(ControlMessage::Acknowledge { .. }) => {}
                    unexpected => panic!("unexpected feedback: {unexpected:?}"),
                }
            }
            let close = ControlMessage::Close {
                reason_code: 1,
                detail: "test".into(),
            };
            control_peer
                .write_all(
                    &ad_stream_protocol::encode_frame(&ad_stream_protocol::ArppFrame::new(
                        9,
                        ArppMessage::Control(close.clone()),
                    ))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(channels.control.recv().await.unwrap(), close);
        })
        .await
        .expect("video backlog blocked control");
    }

    #[tokio::test]
    async fn stopped_demuxer_cannot_consume_close_acknowledgement() {
        use crate::{SshChannelIo, SshTransportAdapter};
        use tokio::io::{duplex, AsyncWriteExt};
        let (control, mut peer) = duplex(4096);
        let (input, _input_peer) = duplex(1024);
        let (media, _media_peer) = duplex(1024);
        let session = Arc::new(ArppSession::new(Arc::new(SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        ))));
        let (mut demuxer, _channels) = StreamDemuxer::spawn(session.clone());
        tokio::task::yield_now().await;
        demuxer.shutdown_and_wait().await;
        let reply = ad_stream_protocol::ArppFrame::new(
            1,
            ArppMessage::Control(ControlMessage::Close {
                reason_code: 0,
                detail: "ended".into(),
            }),
        );
        peer.write_all(&ad_stream_protocol::encode_frame(&reply).unwrap())
            .await
            .unwrap();
        assert_eq!(session.receive().await.unwrap(), reply);
    }

    #[test]
    fn saturated_display_queue_drops_media_without_blocking() {
        let (sender, _receiver) = mpsc::channel(1);
        assert!(enqueue(&sender, 1_u8, "display", true).is_ok());
        assert!(enqueue(&sender, 2_u8, "display", true).is_ok());
    }

    #[test]
    fn saturated_control_queue_reports_overload() {
        let (sender, _receiver) = mpsc::channel(1);
        assert!(enqueue(&sender, 1_u8, "control", false).is_ok());
        assert_eq!(enqueue(&sender, 2_u8, "control", false), Err("control"));
    }

    #[tokio::test]
    async fn overloaded_video_resumes_only_from_a_new_keyframe() {
        let keyframe = DisplayMessage::EncodedFrame {
            frame_id: 1,
            presentation_time_us: 1,
            codec: VideoCodec::H264,
            width: 16,
            height: 16,
            key_frame: true,
            data: vec![1],
        };
        let delta = DisplayMessage::EncodedFrame {
            frame_id: 2,
            presentation_time_us: 2,
            codec: VideoCodec::H264,
            width: 16,
            height: 16,
            key_frame: false,
            data: vec![2],
        };
        let (sender, mut receiver) = mpsc::channel(1);
        let feedback = Feedback::default();
        let mut recovering = false;
        assert!(
            !queue_display(&sender, keyframe.clone(), &mut recovering, &feedback)
                .await
                .unwrap()
        );
        assert!(
            queue_display(&sender, delta.clone(), &mut recovering, &feedback)
                .await
                .unwrap()
        );
        assert!(recovering);
        assert_eq!(feedback.dropped.load(Ordering::Relaxed), 1);
        receiver.recv().await.unwrap();
        assert!(
            !queue_display(&sender, delta.clone(), &mut recovering, &feedback)
                .await
                .unwrap()
        );
        assert!(receiver.try_recv().is_err());
        assert!(
            !queue_display(&sender, keyframe.clone(), &mut recovering, &feedback)
                .await
                .unwrap()
        );
        assert!(!recovering);
        assert_eq!(receiver.recv().await.unwrap(), keyframe);
        assert!(
            !queue_display(&sender, delta.clone(), &mut recovering, &feedback)
                .await
                .unwrap()
        );
        assert_eq!(receiver.recv().await.unwrap(), delta);
    }
}
