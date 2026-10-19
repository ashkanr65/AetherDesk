//! Local LSP stdio with bounded framing and process cleanup. SSH uses the
//! production authenticated channel; no provider API or terminal is involved.
use ad_ssh::LanguageServer;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
pub enum Transport {
    Remote(LanguageServer),
    Local(Local),
}
pub struct Local {
    tx: mpsc::Sender<Vec<u8>>,
    rx: std::sync::Mutex<mpsc::Receiver<Result<Value, String>>>,
    cancel: CancellationToken,
}
impl Drop for Local {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Transport {
    pub fn send(&self, value: Value) -> Result<(), String> {
        match self {
            Self::Remote(server) => server.send(value).map_err(|e| e.to_string()),
            Self::Local(server) => {
                let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err("Language message exceeds 2 MiB".into());
                }
                server
                    .tx
                    .try_send(bytes)
                    .map_err(|_| "Language server input is full or stopped".into())
            }
        }
    }
    pub fn poll(&self) -> Value {
        match self {
            Self::Remote(server) => server.poll(),
            Self::Local(server) => {
                let mut rx = server.rx.lock().unwrap();
                let mut messages = Vec::new();
                let mut error = None;
                let mut stopped = false;
                for _ in 0..64 {
                    match rx.try_recv() {
                        Ok(Ok(value)) => messages.push(value),
                        Ok(Err(e)) => {
                            error = Some(e);
                            break;
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            stopped = true;
                            break;
                        }
                    }
                }
                json!({"state":if error.is_some(){"failed"}else if stopped{"stopped"}else{"running"},"messages":messages,"error":error})
            }
        }
    }
    pub fn stop(&self) {
        match self {
            Self::Remote(server) => server.stop(),
            Self::Local(server) => server.cancel.cancel(),
        }
    }
    pub async fn local(
        executable: String,
        arguments: Vec<String>,
        directory: std::path::PathBuf,
    ) -> Result<Self, String> {
        if executable.is_empty()
            || executable.starts_with('-')
            || executable.contains('\0')
            || arguments.len() > 64
            || arguments
                .iter()
                .any(|s| s.len() > 16384 || s.contains('\0'))
        {
            return Err("Invalid language server executable or arguments".into());
        }
        let mut command = tokio::process::Command::new(executable);
        command
            .args(arguments)
            .current_dir(directory)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let job = crate::process_job::ProcessJob::attach(&child)?;
        let mut input = child.stdin.take().ok_or("Missing language server stdin")?;
        let mut output = child
            .stdout
            .take()
            .ok_or("Missing language server stdout")?;
        let (tx, mut outgoing) = mpsc::channel::<Vec<u8>>(32);
        let (sender, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let cancellation = cancel.clone();
        tokio::spawn(async move {
            let writer = async {
                while let Some(bytes) = outgoing.recv().await {
                    input
                        .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
                        .await
                        .map_err(|e| e.to_string())?;
                    input.write_all(&bytes).await.map_err(|e| e.to_string())?;
                    input.flush().await.map_err(|e| e.to_string())?;
                }
                Ok::<(), String>(())
            };
            let reader = async {
                loop {
                    let value = read_message(&mut output).await?;
                    sender
                        .send(Ok(value))
                        .await
                        .map_err(|_| "Editor closed".to_owned())?;
                }
            };
            let result: Result<(), String> = tokio::select! {_=cancellation.cancelled()=>Ok(()),result=writer=>result,result=reader=>result,result=child.wait()=>Err(format!("Language server exited: {result:?}"))};
            if let Err(error) = result {
                let _ = sender.try_send(Err(error));
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
            #[cfg(windows)]
            drop(job);
        });
        Ok(Self::Local(Local {
            tx,
            rx: std::sync::Mutex::new(rx),
            cancel,
        }))
    }
}
async fn read_message(output: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Value, String> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= 8192 {
            return Err("Language server header is too large".into());
        }
        header.push(output.read_u8().await.map_err(|e| e.to_string())?);
    }
    let header = std::str::from_utf8(&header).map_err(|e| e.to_string())?;
    let mut lengths = header
        .lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"));
    let length = lengths
        .next()
        .ok_or("Missing Content-Length")?
        .1
        .trim()
        .parse::<usize>()
        .map_err(|e| e.to_string())?;
    if lengths.next().is_some() || length > 2 * 1024 * 1024 {
        return Err("Invalid language server message length".into());
    }
    let mut bytes = vec![0; length];
    output
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if !value.is_object() {
        return Err("Language message must be an object".into());
    }
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn fragmented_frames_preserve_unicode_and_reject_duplicate_lengths() {
        let (mut write, mut read) = tokio::io::duplex(4);
        let bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","result":"😀"})).unwrap();
        let frame = [
            format!("Content-Length: {}\r\n\r\n", bytes.len()).into_bytes(),
            bytes,
        ]
        .concat();
        tokio::spawn(async move {
            for byte in frame {
                write.write_all(&[byte]).await.unwrap();
            }
        });
        assert_eq!(read_message(&mut read).await.unwrap()["result"], "😀");
        let mut invalid =
            std::io::Cursor::new(b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec());
        assert!(read_message(&mut invalid).await.is_err());
        let mut oversized = std::io::Cursor::new(b"Content-Length: 999999999\r\n\r\n".to_vec());
        assert!(read_message(&mut oversized).await.is_err());
    }
}
