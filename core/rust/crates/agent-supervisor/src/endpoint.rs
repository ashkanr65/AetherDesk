//! Read-only health checks and narrowly scoped recovery of retained desktops.
use std::io::{Error, ErrorKind};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ad_stream_client::{ArppSession, SshChannelIo, SshTransportAdapter};
use ad_stream_protocol::{ArppMessage, ControlMessage};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::AgentServerEndpoint;

const PREFIX: &str = "AETHERDESK_AGENT_V1";
const HEALTH_PREFIX: &str = "AETHERDESK_HEALTH_V1";
const MAX_LOG_TAIL: u64 = 64 * 1024;

pub fn endpoint_text(endpoint: &AgentServerEndpoint) -> String {
    format!("{PREFIX} {} {}", endpoint.port, hex(&endpoint.token))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

fn parse(text: &str) -> Option<AgentServerEndpoint> {
    let mut fields = text.split_ascii_whitespace();
    if fields.next()? != PREFIX {
        return None;
    }
    let port = fields.next()?.parse().ok()?;
    let encoded = fields.next()?;
    if port == 0 || encoded.len() != 64 || fields.next().is_some() {
        return None;
    }
    let mut token = [0; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        token[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    if token == [0; 32] {
        return None;
    }
    Some(AgentServerEndpoint { port, token })
}

async fn publish(path: &Path, text: &str) -> Result<(), Error> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .await?;
    file.write_all(text.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, path).await
}

pub async fn publish_health(
    directory: &Path,
    endpoint: &AgentServerEndpoint,
    failed: bool,
) -> Result<(), Error> {
    publish(
        &directory.join("health"),
        &format!(
            "{HEALTH_PREFIX} {} {}\n",
            hex(&endpoint.token),
            if failed { "failed" } else { "ready" }
        ),
    )
    .await
}

pub async fn publish_ready(directory: &Path, endpoint: &AgentServerEndpoint) -> Result<(), Error> {
    publish_health(directory, endpoint, false).await?;
    publish(&directory.join("endpoint"), &endpoint_text(endpoint)).await
}

/// Called by the independent watcher after it has reaped the broker. A stale
/// ready record must never claim a dead desktop can still be resumed.
pub async fn record_process_exit(directory: &Path, record: &str) -> Result<(), Error> {
    publish(&directory.join("agent-exit"), record).await?;
    let endpoint = match tokio::fs::read_to_string(directory.join("endpoint")).await {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(endpoint) = endpoint {
        publish_health(directory, &endpoint, true).await?;
    }
    Ok(())
}

// A legacy broker has no health record. Only explicit terminal failure from
// the supervisor/compositor after its latest startup is grounds for recovery.
// Missing evidence, arbitrary warnings, EOF and timeouts are not proof.
fn legacy_failed(text: &str) -> bool {
    let mut failed = false;
    for line in text.lines() {
        if line.contains(" INFO agent_supervisor::supervisor: persistent AetherDesk session started") {
            failed = false;
        } else if line.contains(" ERROR agent_wayland::compositor: headless compositor stopped error=")
            || line.contains(" ERROR agent_supervisor::supervisor: headless compositor capture loop failed error=compositor failed: the compositor has stopped")
            || line.contains(" WARN agent_supervisor::server: client dropped; retaining remote session error=agent supervisor failed: the persistent agent has stopped")
        {
            failed = true;
        }
    }
    failed
}

async fn failed(directory: &Path, endpoint: &AgentServerEndpoint) -> Result<bool, Error> {
    match tokio::fs::read_to_string(directory.join("health")).await {
        Ok(record) => {
            let ready = format!("{HEALTH_PREFIX} {} ready", hex(&endpoint.token));
            let failed = format!("{HEALTH_PREFIX} {} failed", hex(&endpoint.token));
            if record.trim() == ready {
                return Ok(false);
            }
            if record.trim() == failed {
                return Ok(true);
            }
            return Err(Error::other(
                "Retained desktop health does not match its endpoint; leaving it unchanged",
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = match tokio::fs::File::open(directory.join("agent.log")).await {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let start = file.metadata().await?.len().saturating_sub(MAX_LOG_TAIL);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut tail = Vec::new();
    file.take(MAX_LOG_TAIL).read_to_end(&mut tail).await?;
    let text = String::from_utf8_lossy(&tail);
    // Do not interpret a partial first line from a bounded tail.
    Ok(legacy_failed(if start == 0 {
        &text
    } else {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    }))
}

async fn connect(endpoint: &AgentServerEndpoint) -> Result<ArppSession, Error> {
    let mut channels = Vec::new();
    for channel in 0..3 {
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)).await?;
        let mut prelude = [0; 38];
        prelude[..4].copy_from_slice(b"ADCH");
        prelude[4] = 1;
        prelude[5] = channel;
        prelude[6..].copy_from_slice(&endpoint.token);
        stream.write_all(&prelude).await?;
        channels.push(stream);
    }
    let mut channels = channels.into_iter();
    Ok(ArppSession::new(Arc::new(SshTransportAdapter::new(
        SshChannelIo::from_stream(channels.next().expect("control")),
        SshChannelIo::from_stream(channels.next().expect("input")),
        SshChannelIo::from_stream(channels.next().expect("media")),
    ))))
}

async fn retire_failed(
    directory: &Path,
    endpoint: &AgentServerEndpoint,
    cleanup_timeout: Duration,
) -> Result<(), Error> {
    // Recheck evidence and endpoint identity immediately before requesting
    // termination. No process-name kills, PID signals or guessed displays.
    let current = match tokio::fs::read_to_string(directory.join("endpoint")).await {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return wait_for_cleanup(directory, cleanup_timeout).await;
        }
        Err(error) => return Err(error),
    };
    if !failed(directory, endpoint).await? || current.trim() != endpoint_text(endpoint) {
        return Err(Error::other(
            "Desktop changed during recovery; leaving it unchanged",
        ));
    }
    tracing::warn!(
        port = endpoint.port,
        "recovering a confirmed failed retained desktop"
    );
    // Close is valid before ClientHello, including in legacy brokers whose
    // broken pipeline rejects the normal hello. Never claim/attach the desktop.
    let _ = timeout(Duration::from_secs(3), async {
        let session = connect(endpoint).await?;
        session
            .send(ArppMessage::Control(ControlMessage::Close {
                reason_code: 0,
                detail: "Recovering confirmed failed desktop".into(),
            }))
            .await
            .map_err(Error::other)?;
        let _ = session.receive().await;
        Ok::<_, Error>(())
    })
    .await;
    // A modern broker may already be cleaning up. Legacy EOF/ack alone is
    // insufficient: wait for removal that occurs after supervisor shutdown.
    wait_for_cleanup(directory, cleanup_timeout).await
}

async fn wait_for_cleanup(directory: &Path, cleanup_timeout: Duration) -> Result<(), Error> {
    timeout(cleanup_timeout, async {
        loop {
            match tokio::fs::symlink_metadata(directory).await {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
                Ok(_) => {}
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| {
        Error::other("Failed desktop cleanup could not be confirmed; no replacement was started")
    })?
}

pub async fn read_live_endpoint(path: &Path) -> Result<Option<String>, Error> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(value) => value,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(endpoint) = parse(&text) else {
        return Err(Error::other(
            "Retained desktop endpoint is invalid; leaving it unchanged",
        ));
    };
    let directory = path
        .parent()
        .ok_or_else(|| Error::other("Missing session directory"))?;
    if failed(directory, &endpoint).await? {
        retire_failed(directory, &endpoint, Duration::from_secs(7)).await?;
        return Ok(None);
    }
    if !matches!(
        timeout(
            Duration::from_secs(1),
            TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port))
        )
        .await,
        Ok(Ok(_))
    ) {
        return Ok(None);
    }
    // Shutdown may finish between the health read and the TCP check.
    match tokio::fs::read_to_string(path).await {
        Ok(current) if current.trim() == text.trim() => Ok(Some(endpoint_text(&endpoint))),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
        _ => Err(Error::other(
            "Retained desktop endpoint changed during health check",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn failed_broker(
        listener: TcpListener,
        endpoint: &AgentServerEndpoint,
        directory: &Path,
        clean: bool,
    ) -> tokio::task::JoinHandle<()> {
        let expected_token = endpoint.token;
        let cleanup = directory.to_owned();
        tokio::spawn(async move {
            let mut channels = Vec::new();
            for id in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut prelude = [0; 38];
                stream.read_exact(&mut prelude).await.unwrap();
                assert_eq!(&prelude[..6], &[b'A', b'D', b'C', b'H', 1, id]);
                assert_eq!(&prelude[6..], &expected_token);
                channels.push(stream);
            }
            let mut channels = channels.into_iter();
            let session = ArppSession::new(Arc::new(SshTransportAdapter::new(
                SshChannelIo::from_stream(channels.next().unwrap()),
                SshChannelIo::from_stream(channels.next().unwrap()),
                SshChannelIo::from_stream(channels.next().unwrap()),
            )));
            let message = session.receive().await.unwrap().message;
            assert!(matches!(
                message,
                ArppMessage::Control(ControlMessage::Close { reason_code: 0, .. })
            ));
            // EOF by itself must not allow a replacement to race shutdown.
            drop(session);
            tokio::time::sleep(Duration::from_millis(100)).await;
            if clean {
                tokio::fs::remove_dir_all(cleanup).await.unwrap();
            }
        })
    }

    #[test]
    fn legacy_recovery_requires_terminal_failure_after_latest_start() {
        let started = " INFO agent_supervisor::supervisor: persistent AetherDesk session started";
        let stopped = " WARN agent_supervisor::server: client dropped; retaining remote session error=agent supervisor failed: the persistent agent has stopped";
        assert!(!legacy_failed("DPMS missing\nSSH unexpected end of file\n"));
        assert!(legacy_failed(&format!("{started}\n{stopped}\n")));
        assert!(!legacy_failed(&format!("{stopped}\n{started}\n")));
        assert!(!legacy_failed(
            " INFO application: the persistent agent has stopped"
        ));
    }
    #[tokio::test]
    async fn reaped_broker_exit_is_recorded_and_invalidates_stale_ready_health() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = AgentServerEndpoint {
            port: 12345,
            token: [7; 32],
        };
        publish_ready(root.path(), &endpoint).await.unwrap();
        record_process_exit(root.path(), "AETHERDESK_AGENT_EXIT_V1 signal=Some(15)\n")
            .await
            .unwrap();
        assert!(tokio::fs::read_to_string(root.path().join("agent-exit"))
            .await
            .unwrap()
            .contains("signal=Some(15)"));
        assert!(tokio::fs::read_to_string(root.path().join("health"))
            .await
            .unwrap()
            .trim()
            .ends_with("failed"));
    }

    #[tokio::test]
    async fn healthy_retained_endpoint_is_reused_without_attaching_or_terminating() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentServerEndpoint {
            port: listener.local_addr().unwrap().port(),
            token: [7; 32],
        };
        publish_ready(root.path(), &endpoint).await.unwrap();
        let result = read_live_endpoint(&root.path().join("endpoint"))
            .await
            .unwrap();
        assert_eq!(result, Some(endpoint_text(&endpoint)));
        let (mut probe, _) = listener.accept().await.unwrap();
        let mut byte = [0];
        assert_eq!(
            probe.read(&mut byte).await.unwrap(),
            0,
            "health check sent a desktop command"
        );
        assert!(root.path().exists());
    }

    #[tokio::test]
    async fn mismatched_health_never_terminates_another_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = AgentServerEndpoint {
            port: 1234,
            token: [7; 32],
        };
        publish_ready(root.path(), &endpoint).await.unwrap();
        publish_health(
            root.path(),
            &AgentServerEndpoint {
                port: 1234,
                token: [8; 32],
            },
            true,
        )
        .await
        .unwrap();
        assert!(read_live_endpoint(&root.path().join("endpoint"))
            .await
            .is_err());
        assert!(root.path().join("endpoint").exists());
    }

    #[tokio::test]
    async fn legacy_failed_broker_is_closed_and_cleanup_confirmed_before_replacement() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("session");
        tokio::fs::create_dir(&directory).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentServerEndpoint {
            port: listener.local_addr().unwrap().port(),
            token: [7; 32],
        };
        tokio::fs::write(directory.join("endpoint"), endpoint_text(&endpoint))
            .await
            .unwrap();
        tokio::fs::write(directory.join("agent.log"), "2026-10-27T09:29:20Z WARN agent_supervisor::server: client dropped; retaining remote session error=agent supervisor failed: the persistent agent has stopped\n").await.unwrap();
        let task = failed_broker(listener, &endpoint, &directory, true);
        assert_eq!(
            read_live_endpoint(&directory.join("endpoint"))
                .await
                .unwrap(),
            None
        );
        assert!(!directory.exists());
        task.await.unwrap();
    }
    #[tokio::test]
    async fn modern_failed_health_recovers_but_eof_without_cleanup_never_allows_replacement() {
        for clean in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("session");
            tokio::fs::create_dir(&directory).await.unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = AgentServerEndpoint {
                port: listener.local_addr().unwrap().port(),
                token: [9; 32],
            };
            publish_ready(&directory, &endpoint).await.unwrap();
            publish_health(&directory, &endpoint, true).await.unwrap();
            let task = failed_broker(listener, &endpoint, &directory, clean);
            let result = retire_failed(&directory, &endpoint, Duration::from_millis(250)).await;
            assert_eq!(result.is_ok(), clean);
            assert_eq!(directory.exists(), !clean);
            if !clean {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("no replacement was started"));
                assert_eq!(
                    tokio::fs::read_to_string(directory.join("endpoint"))
                        .await
                        .unwrap(),
                    endpoint_text(&endpoint)
                );
            }
            task.await.unwrap();
        }
    }
}
