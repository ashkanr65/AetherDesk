use std::collections::VecDeque;
use std::sync::Arc;

use ad_core::{CoreError, Result};
use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const INPUT_LIMIT: usize = 1024 * 1024;

enum Command {
    Data(Vec<u8>),
    Resize(u16, u16),
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    error: Option<String>,
    ready: Arc<Notify>,
}
impl Output {
    fn append(&mut self, bytes: &[u8]) -> bool {
        if self.bytes.len().saturating_add(bytes.len()) > OUTPUT_LIMIT {
            self.fail("Terminal output exceeded its buffer; reconnect the terminal".into());
            return false;
        }
        self.bytes.extend(bytes.iter().copied());
        self.ready.notify_one();
        true
    }
    fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.ready.notify_one();
    }
}

pub(crate) struct ShellIo {
    commands: mpsc::Sender<Command>,
    output: Arc<Mutex<Output>>,
}

impl ShellIo {
    pub fn spawn(channel: russh::Channel<russh::client::Msg>, cancel: CancellationToken) -> Self {
        let (mut reader, writer) = channel.split();
        let (commands, mut receive) = mpsc::channel(64);
        let output = Arc::new(Mutex::new(Output::default()));
        let reader_output = output.clone();
        let reader_cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                let message = tokio::select! {
                    _ = reader_cancel.cancelled() => break,
                    message = reader.wait() => message,
                };
                let mut output = reader_output.lock();
                match message {
                    Some(
                        russh::ChannelMsg::Data { data }
                        | russh::ChannelMsg::ExtendedData { data, .. },
                    ) => {
                        if !output.append(&data) {
                            break;
                        }
                    }
                    Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close) | None => {
                        output.fail("The remote shell closed the terminal channel".into());
                        break;
                    }
                    _ => {}
                }
            }
            // A stopped reader must stop the writer too (including overflow),
            // releasing the PTY instead of leaving a shell with no consumer.
            reader_cancel.cancel();
            reader_output.lock().ready.notify_one();
        });
        let writer_output = output.clone();
        tokio::spawn(async move {
            loop {
                let result = tokio::select! {
                    _ = cancel.cancelled() => break,
                    result = async {
                        match receive.recv().await {
                            Some(Command::Data(bytes)) => writer.data_bytes(bytes).await,
                            Some(Command::Resize(cols, rows)) => writer.window_change(cols.into(), rows.into(), 0, 0).await,
                            None => return None,
                        }.into()
                    } => result,
                };
                match result {
                    Some(Ok(())) => {}
                    Some(Err(error)) => {
                        writer_output.lock().fail(error.to_string());
                        break;
                    }
                    None => break,
                }
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                let _ = writer.eof().await;
                writer.close().await
            })
            .await;
        });
        Self { commands, output }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > INPUT_LIMIT {
            return Err(CoreError::ConnectionError(
                "Terminal paste exceeds 1 MiB".into(),
            ));
        }
        self.enqueue(Command::Data(bytes.to_vec()))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.enqueue(Command::Resize(cols, rows))
    }

    fn enqueue(&self, command: Command) -> Result<()> {
        if let Some(error) = &self.output.lock().error {
            return Err(CoreError::ConnectionError(error.clone()));
        }
        self.commands.try_send(command).map_err(|error| {
            CoreError::ConnectionError(format!("Terminal input could not be queued: {error}"))
        })
    }

    pub fn read(&self, limit: usize) -> Result<Vec<u8>> {
        let mut output = self.output.lock();
        if output.bytes.is_empty() {
            if let Some(error) = &output.error {
                return Err(CoreError::ConnectionError(error.clone()));
            }
        }
        let len = output.bytes.len().min(limit);
        Ok(output.bytes.drain(..len).collect())
    }

    pub(crate) fn output_readiness(&self) -> (bool, Arc<Notify>) {
        let output = self.output.lock();
        (
            !output.bytes.is_empty() || output.error.is_some(),
            output.ready.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_notification_survives_arrival_before_wait_and_drains_before_close() {
        let (commands, _receive) = mpsc::channel(2);
        let shell = ShellIo {
            commands,
            output: Arc::new(Mutex::new(Output::default())),
        };
        let (pending, signal) = shell.output_readiness();
        assert!(!pending);
        // Bytes may arrive between the consumer's poll and awaiting the signal.
        assert!(shell.output.lock().append(b"first second"));
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.notified())
            .await
            .unwrap();
        assert_eq!(shell.read(6).unwrap(), b"first ");
        assert!(
            shell.output_readiness().0,
            "partial drains must not wait for another packet"
        );
        shell.output.lock().fail("closed".into());
        assert_eq!(shell.read(100).unwrap(), b"second");
        assert!(shell.read(100).is_err());
        assert!(
            shell.output_readiness().0,
            "closure must wake a waiting terminal"
        );
    }

    #[test]
    fn preserves_binary_input_and_output_before_reporting_close() {
        let (commands, mut receive) = mpsc::channel(2);
        let shell = ShellIo {
            commands,
            output: Arc::new(Mutex::new(Output::default())),
        };
        shell.write(&[0, 3, 0xff]).unwrap();
        assert!(
            matches!(receive.try_recv().unwrap(), Command::Data(bytes) if bytes == [0, 3, 0xff])
        );
        shell.output.lock().bytes.extend([0xe2, 0x82, 0xac, 0]);
        shell.output.lock().error = Some("closed".into());
        assert_eq!(shell.read(2).unwrap(), [0xe2, 0x82]);
        assert_eq!(shell.read(2).unwrap(), [0xac, 0]);
        assert!(shell.read(2).is_err());
    }

    #[test]
    fn saturated_writer_reports_failure_without_waiting() {
        let (commands, _receive) = mpsc::channel(1);
        let shell = ShellIo {
            commands,
            output: Arc::new(Mutex::new(Output::default())),
        };
        shell.write(b"a").unwrap();
        assert!(shell.write(b"b").is_err());
        assert!(shell.read(65536).unwrap().is_empty());
    }
}
