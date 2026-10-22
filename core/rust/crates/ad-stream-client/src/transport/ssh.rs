use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ad_stream_protocol::{decode_frame, encode_frame, ArppFrame, SshChannelId};
use async_trait::async_trait;
use bytes::{Buf, Bytes, BytesMut};
use tokio::io::{split, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use super::{PriorityInbox, TransportAdapter, TransportError, TransportKind};

type BoxReader = Pin<Box<dyn AsyncRead + Send>>;
type BoxWriter = Pin<Box<dyn AsyncWrite + Send>>;

pub struct SshChannelIo {
    reader: BoxReader,
    writer: BoxWriter,
}

impl SshChannelIo {
    pub fn from_stream<T>(stream: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (reader, writer) = split(stream);
        Self {
            reader: Box::pin(reader),
            writer: Box::pin(writer),
        }
    }

    pub fn from_russh(channel: russh::Channel<russh::client::Msg>) -> Self {
        Self::from_stream(channel.into_stream())
    }
}

// Keep progress in the transport, not in the sending future. In particular,
// aborting the demuxer's receipt task during shutdown must not truncate a
// control frame and corrupt the Close request that follows it.
struct ChannelWriter {
    writer: BoxWriter,
    pending: Bytes,
}

impl ChannelWriter {
    fn new(writer: BoxWriter) -> Self {
        Self {
            writer,
            pending: Bytes::new(),
        }
    }

    async fn finish_pending(&mut self) -> Result<(), std::io::Error> {
        while !self.pending.is_empty() {
            // AsyncWriteExt::write is cancellation safe. Advance the saved
            // buffer only after the writer reports the accepted byte count.
            let written = self.writer.write(&self.pending).await?;
            if written == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.pending.advance(written);
        }
        self.writer.flush().await
    }
}

pub struct SshTransportAdapter {
    control: Mutex<ChannelWriter>,
    input: Mutex<ChannelWriter>,
    media: Mutex<ChannelWriter>,
    inbox: Arc<PriorityInbox>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    ending: Arc<AtomicBool>,
}

impl SshTransportAdapter {
    pub fn new(control: SshChannelIo, input: SshChannelIo, media: SshChannelIo) -> Self {
        let inbox = Arc::new(PriorityInbox::new());
        let ending = Arc::new(AtomicBool::new(false));
        let readers = vec![
            spawn_reader(
                control.reader,
                SshChannelId::Control,
                Arc::clone(&inbox),
                Arc::clone(&ending),
            ),
            spawn_reader(
                input.reader,
                SshChannelId::Input,
                Arc::clone(&inbox),
                Arc::clone(&ending),
            ),
            spawn_reader(
                media.reader,
                SshChannelId::Media,
                Arc::clone(&inbox),
                Arc::clone(&ending),
            ),
        ];
        Self {
            control: Mutex::new(ChannelWriter::new(control.writer)),
            input: Mutex::new(ChannelWriter::new(input.writer)),
            media: Mutex::new(ChannelWriter::new(media.writer)),
            inbox,
            readers,
            ending,
        }
    }

    async fn write(&self, channel: SshChannelId, frame: &ArppFrame) -> Result<(), TransportError> {
        let encoded = encode_frame(frame)?;
        let writer = match channel {
            SshChannelId::Control => &self.control,
            SshChannelId::Input => &self.input,
            SshChannelId::Media => &self.media,
        };
        let mut writer = writer.lock().await;
        // A cancelled send may have put only part of a frame on the wire.
        // Complete it before starting another frame on this same channel.
        writer.finish_pending().await?;
        writer.pending = encoded;
        writer.finish_pending().await?;
        Ok(())
    }
}

#[async_trait]
impl TransportAdapter for SshTransportAdapter {
    fn kind(&self) -> TransportKind {
        TransportKind::SshChannels
    }

    async fn send(&self, frame: ArppFrame) -> Result<(), TransportError> {
        if matches!(
            &frame.message,
            ad_stream_protocol::ArppMessage::Control(ad_stream_protocol::ControlMessage::Close {
                reason_code: 0,
                ..
            })
        ) {
            self.ending.store(true, Ordering::Release);
        }
        self.write(frame.class().ssh_channel(), &frame).await
    }

    async fn receive(&self) -> Result<ArppFrame, TransportError> {
        self.inbox.receive().await
    }

    async fn close(&self, _reason: &str) -> Result<(), TransportError> {
        for task in &self.readers {
            task.abort();
        }
        self.control.lock().await.writer.shutdown().await?;
        self.input.lock().await.writer.shutdown().await?;
        self.media.lock().await.writer.shutdown().await?;
        Ok(())
    }
}

impl Drop for SshTransportAdapter {
    fn drop(&mut self) {
        for task in &self.readers {
            task.abort();
        }
    }
}

fn spawn_reader(
    mut reader: BoxReader,
    expected: SshChannelId,
    inbox: Arc<PriorityInbox>,
    ending: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let result = read_frame(&mut reader).await.and_then(|frame| {
                if frame.class().ssh_channel() != expected {
                    return Err(TransportError::WrongSshChannel(expected as u8));
                }
                Ok(frame)
            });
            let should_stop = result.is_err();
            // End-session confirmation travels on control after host cleanup.
            // The server can stop a partial media write during that cleanup;
            // its EOF must not race ahead of the authoritative control reply.
            if should_stop && expected != SshChannelId::Control && ending.load(Ordering::Acquire) {
                break;
            }
            let result = result.map_err(|error| match error {
                TransportError::Io(error) => TransportError::Io(std::io::Error::new(
                    error.kind(),
                    format!("SSH {expected:?} channel: {error}"),
                )),
                other => other,
            });
            inbox.senders.send(result).await;
            if should_stop {
                break;
            }
        }
    })
}

pub(super) async fn read_frame<R>(reader: &mut R) -> Result<ArppFrame, TransportError>
where
    R: AsyncRead + Unpin + ?Sized,
{
    let body_len = reader.read_u32().await? as usize;
    if body_len > ad_stream_protocol::MAX_FRAME_BODY_LEN {
        return Err(ad_stream_protocol::ProtocolError::PayloadTooLarge(body_len).into());
    }
    let mut buffer = BytesMut::with_capacity(4 + body_len);
    buffer.extend_from_slice(&(body_len as u32).to_be_bytes());
    buffer.resize(4 + body_len, 0);
    reader.read_exact(&mut buffer[4..]).await?;
    decode_frame(&mut buffer)?.ok_or(TransportError::Closed)
}

#[cfg(test)]
mod tests {
    use ad_stream_protocol::{ArppMessage, DisplayMessage, InputMessage, Rectangle};
    use tokio::io::duplex;

    use super::*;

    #[tokio::test]
    async fn cancelled_control_write_does_not_corrupt_the_end_request() {
        use ad_stream_protocol::ControlMessage;
        use std::time::Duration;

        // Backpressure interrupts a receipt halfway through its frame, as when
        // stopping the demuxer's receipt task before sending End session.
        let (control, mut peer) = duplex(8);
        let (input, _input_peer) = duplex(8);
        let (media, _media_peer) = duplex(8);
        let adapter = SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        );
        let receipt = ArppFrame::new(
            1,
            ArppMessage::Control(ControlMessage::Acknowledge { sequence: 42 }),
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(25), adapter.send(receipt.clone()))
                .await
                .is_err()
        );
        let end = ArppFrame::new(
            2,
            ArppMessage::Control(ControlMessage::Close {
                reason_code: 0,
                detail: "End session".into(),
            }),
        );
        let reader = tokio::spawn(async move {
            let first = read_frame(&mut peer).await.unwrap();
            let second = read_frame(&mut peer).await.unwrap();
            (first, second)
        });
        tokio::time::timeout(Duration::from_secs(1), adapter.send(end.clone()))
            .await
            .unwrap()
            .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, (receipt, end));
    }

    #[tokio::test]
    async fn ending_waits_for_control_confirmation_when_media_closes_mid_frame() {
        let (control, mut control_peer) = duplex(65536);
        let (input, _input_peer) = duplex(1024);
        let (media, mut media_peer) = duplex(1024);
        let adapter = SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        );
        let confirmation = ArppFrame::new(
            1,
            ArppMessage::Control(ad_stream_protocol::ControlMessage::Close {
                reason_code: 0,
                detail: "Ended".into(),
            }),
        );
        adapter.send(confirmation.clone()).await.unwrap();
        media_peer.write_all(&[0, 0, 1, 0, 1, 2]).await.unwrap();
        drop(media_peer);
        tokio::task::yield_now().await;
        control_peer
            .write_all(&encode_frame(&confirmation).unwrap())
            .await
            .unwrap();
        drop(control_peer);
        assert_eq!(adapter.receive().await.unwrap(), confirmation);
    }

    #[tokio::test]
    async fn dropping_adapter_releases_all_channel_readers() {
        let (control, mut peer) = duplex(1024);
        let (input, _input_peer) = duplex(1024);
        let (media, _media_peer) = duplex(1024);
        let adapter = SshTransportAdapter::new(
            SshChannelIo::from_stream(control),
            SshChannelIo::from_stream(input),
            SshChannelIo::from_stream(media),
        );
        drop(adapter);
        let mut buffer = [0u8; 1];
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), peer.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(peer.write_all(&[0]).await.is_err());
    }

    #[tokio::test]
    async fn routes_input_to_a_channel_independent_of_display() {
        let (control_client, _control_server) = duplex(64 * 1024);
        let (input_client, mut input_server) = duplex(64 * 1024);
        let (media_client, mut media_server) = duplex(64 * 1024);
        let adapter = SshTransportAdapter::new(
            SshChannelIo::from_stream(control_client),
            SshChannelIo::from_stream(input_client),
            SshChannelIo::from_stream(media_client),
        );

        let display = ArppFrame::new(
            1,
            ArppMessage::Display(DisplayMessage::DamageRegions {
                frame_id: 1,
                regions: vec![Rectangle {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10,
                }],
            }),
        );
        let input = ArppFrame::new(
            2,
            ArppMessage::Input(InputMessage::PointerMove {
                x: 1,
                y: 2,
                monitor: 0,
            }),
        );
        adapter.send(display.clone()).await.unwrap();
        adapter.send(input.clone()).await.unwrap();

        assert_eq!(read_frame(&mut input_server).await.unwrap(), input);
        assert_eq!(read_frame(&mut media_server).await.unwrap(), display);
    }
}
