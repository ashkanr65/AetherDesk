//! Bounded language-server stdio channels on an authenticated SSH transport.
use super::*;
use serde_json::Value;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAX_MESSAGE: usize = 2 * 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LanguageServerConfig {
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
}

impl LanguageServerConfig {
    fn command(&self) -> CoreResult<String> {
        let quote = |value: &str| -> CoreResult<String> {
            if value.contains('\0') || value.len() > 16_384 {
                return Err(sftp_error("invalid language server argument"));
            }
            Ok(format!("'{}'", value.replace('\'', "'\\''")))
        };
        if self.executable.is_empty()
            || self.executable.starts_with('-')
            || self.arguments.len() > 64
        {
            return Err(sftp_error("invalid language server configuration"));
        }
        let mut command = String::new();
        if let Some(directory) = &self.working_directory {
            if !directory.starts_with('/') {
                return Err(sftp_error(
                    "language server working directory must be absolute",
                ));
            }
            command.push_str(&format!("cd {} && ", quote(directory)?));
        }
        command.push_str(&format!("exec {}", quote(&self.executable)?));
        for argument in &self.arguments {
            command.push_str(&format!(" {}", quote(argument)?));
        }
        if command.len() > 65_536 {
            return Err(sftp_error("language server command is too long"));
        }
        Ok(command)
    }
}

#[derive(Default)]
struct Output {
    messages: VecDeque<(Value, usize)>,
    bytes: usize,
    stopped: bool,
    error: Option<String>,
}

pub struct LanguageServer {
    outgoing: mpsc::Sender<Vec<u8>>,
    output: Arc<parking_lot::Mutex<Output>>,
    cancel: CancellationToken,
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl LanguageServer {
    pub fn send(&self, message: Value) -> CoreResult<()> {
        if !message.is_object() {
            return Err(sftp_error("language server message must be a JSON object"));
        }
        let bytes = serde_json::to_vec(&message).map_err(|e| sftp_error(e.to_string()))?;
        if bytes.len() > MAX_MESSAGE {
            return Err(sftp_error("language server message is too large"));
        }
        let mut framed = format!("Content-Length: {}\r\n\r\n", bytes.len()).into_bytes();
        framed.extend_from_slice(&bytes);
        self.outgoing
            .try_send(framed)
            .map_err(|_| sftp_error("language server is stopped or its input queue is full"))
    }

    pub fn poll(&self) -> Value {
        let mut output = self.output.lock();
        let mut messages = Vec::new();
        while messages.len() < 64 {
            let Some((message, bytes)) = output.messages.pop_front() else {
                break;
            };
            output.bytes -= bytes;
            messages.push(message);
        }
        serde_json::json!({"state": if output.error.is_some() { "failed" } else if output.stopped { "stopped" } else { "running" }, "messages": messages, "error": output.error})
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl SshSession {
    pub async fn start_language_server(
        &self,
        config: LanguageServerConfig,
    ) -> CoreResult<LanguageServer> {
        let command = config.command()?;
        // Bulk traffic remains isolated from terminal input where a dedicated
        // graphics/file transport exists.
        let transport = self.bulk.as_deref().unwrap_or(self);
        let channel = timeout(
            Duration::from_secs(15),
            transport.session.channel_open_session(),
        )
        .await
        .map_err(|_| sftp_error("language server channel timed out"))??;
        let mut channel = OwnedChannel(Some(channel));
        timeout(Duration::from_secs(15), channel.exec(true, command))
            .await
            .map_err(|_| sftp_error("language server launch timed out"))??;
        let (outgoing, mut incoming) = mpsc::channel::<Vec<u8>>(8);
        let output = Arc::new(parking_lot::Mutex::new(Output::default()));
        let worker_output = output.clone();
        let cancel = self.lifetime.0.child_token();
        let worker_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut parser = Framing::default();
            let mut stderr = Vec::new();
            let mut eof = false;
            let result: CoreResult<()> = async {
                loop {
                    tokio::select! {
                        _ = worker_cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(2)), if eof => break,
                        next = incoming.recv() => match next {
                            Some(bytes) => timeout(Duration::from_secs(15), channel.data_bytes(bytes)).await.map_err(|_| sftp_error("language server input stalled"))??,
                            None => break,
                        },
                        message = channel.wait() => match message {
                            Some(russh::ChannelMsg::Data { data }) => {
                                for (message, bytes) in parser.push(&data)? {
                                    let mut output = worker_output.lock();
                                    if output.messages.len() >= 256 || output.bytes + bytes > MAX_QUEUE_BYTES {
                                        return Err(sftp_error("language server output queue exceeded its limit"));
                                    }
                                    output.bytes += bytes;
                                    output.messages.push_back((message, bytes));
                                }
                            }
                            Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                                let available = 8192_usize.saturating_sub(stderr.len());
                                stderr.extend_from_slice(&data[..data.len().min(available)]);
                            }
                            Some(russh::ChannelMsg::ExitStatus { exit_status }) if exit_status != 0 => {
                                return Err(sftp_error(format!("language server exited with status {exit_status}: {}", String::from_utf8_lossy(&stderr))));
                            }
                            Some(russh::ChannelMsg::Failure) => return Err(sftp_error("host refused the language server command")),
                            Some(russh::ChannelMsg::Eof) => eof = true,
                            Some(russh::ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                }
                if !worker_cancel.is_cancelled() && !parser.pending.is_empty() { return Err(sftp_error("language server closed with an incomplete message")); }
                Ok(())
            }.await;
            let _ = timeout(Duration::from_secs(2), async {
                let _ = channel.eof().await;
                channel.close().await
            })
            .await;
            let mut output = worker_output.lock();
            output.stopped = true;
            output.error = result.err().map(|e| e.to_string());
        });
        Ok(LanguageServer {
            outgoing,
            output,
            cancel,
        })
    }
}

#[derive(Default)]
struct Framing {
    pending: Vec<u8>,
}

impl Framing {
    fn push(&mut self, data: &[u8]) -> CoreResult<Vec<(Value, usize)>> {
        if self.pending.len() + data.len() > MAX_MESSAGE + 65_536 {
            return Err(sftp_error("language server frame exceeded its limit"));
        }
        self.pending.extend_from_slice(data);
        let mut result = Vec::new();
        loop {
            let Some(end) = self.pending.windows(4).position(|v| v == b"\r\n\r\n") else {
                if self.pending.len() > 8192 {
                    return Err(sftp_error("invalid language server header"));
                }
                break;
            };
            if end > 8192 {
                return Err(sftp_error("language server header is too long"));
            }
            let header = std::str::from_utf8(&self.pending[..end])
                .map_err(|_| sftp_error("invalid language server header encoding"))?;
            let mut length = None;
            for line in header.split("\r\n") {
                let (name, value) = line
                    .split_once(':')
                    .ok_or_else(|| sftp_error("invalid language server header"))?;
                if name.eq_ignore_ascii_case("Content-Length") {
                    if length.is_some() {
                        return Err(sftp_error("duplicate language server Content-Length"));
                    }
                    length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| sftp_error("invalid language server Content-Length"))?,
                    );
                }
            }
            let length =
                length.ok_or_else(|| sftp_error("missing language server Content-Length"))?;
            if length == 0 || length > MAX_MESSAGE {
                return Err(sftp_error("invalid language server message length"));
            }
            let offset = end + 4;
            if self.pending.len() < offset + length {
                break;
            }
            let message: Value = serde_json::from_slice(&self.pending[offset..offset + length])
                .map_err(|_| sftp_error("language server returned invalid JSON"))?;
            if !message.is_object() {
                return Err(sftp_error("language server returned a non-object message"));
            }
            self.pending.drain(..offset + length);
            result.push((message, length));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_and_concatenated_language_server_frames() {
        let message = serde_json::json!({"jsonrpc":"2.0", "id":1, "result":"ñ"});
        let payload = serde_json::to_vec(&message).unwrap();
        let mut bytes = format!("Content-Length: {}\r\n\r\n", payload.len()).into_bytes();
        bytes.extend(&payload);
        bytes.extend(bytes.clone());
        let mut parser = Framing::default();
        let mut messages = Vec::new();
        for part in bytes.chunks(7) {
            messages.extend(parser.push(part).unwrap());
        }
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].0, message);
        assert_eq!(messages[1].0, message);
    }
    #[test]
    fn language_server_rejects_ambiguous_or_unbounded_frames() {
        for invalid in [
            b"Content-Length: 4\r\nContent-Length: 4\r\n\r\nnull".as_slice(),
            b"Content-Length: 999999999\r\n\r\n",
            b"Content-Length: 4\r\n\r\nnull",
        ] {
            assert!(Framing::default().push(invalid).is_err());
        }
    }
    #[test]
    fn language_server_arguments_are_literal_shell_arguments() {
        let command = LanguageServerConfig {
            executable: "/opt/lsp's/server".into(),
            arguments: vec!["$(touch /tmp/never)".into()],
            working_directory: Some("/project spaces".into()),
        }
        .command()
        .unwrap();
        assert_eq!(
            command,
            "cd '/project spaces' && exec '/opt/lsp'\\''s/server' '$(touch /tmp/never)'"
        );
    }
}
