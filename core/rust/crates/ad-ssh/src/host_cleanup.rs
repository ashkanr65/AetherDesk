//! Temporary state belongs to one SSH connection, never a saved desktop.
use super::*;

#[derive(Default)]
struct SetupReply {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<u32>,
}
impl SetupReply {
    fn ready(&mut self, data: &[u8]) -> CoreResult<Option<String>> {
        self.stdout.extend_from_slice(data);
        if self.stdout.len() > 4096 {
            return Err(CoreError::ConnectionError(
                "Invalid maintenance response".into(),
            ));
        }
        let Some(end) = self.stdout.iter().position(|c| *c == b'\n') else {
            return Ok(None);
        };
        let text = std::str::from_utf8(&self.stdout[..end])
            .map_err(|_| CoreError::ConnectionError("Invalid maintenance response".into()))?;
        text.strip_prefix("AETHERDESK_CLEANUP_READY_V1=")
            .map(|path| Some(path.to_owned()))
            .ok_or_else(|| {
                CoreError::ConnectionError("Invalid maintenance readiness response".into())
            })
    }
    fn diagnostic(&self) -> CoreError {
        // Shell stderr may contain user data. Interpret only our allowlisted
        // codes, never copy arbitrary command output into logs or the UI.
        let stderr = String::from_utf8_lossy(&self.stderr);
        let reason = stderr.lines().find_map(|line| {
            Some(
                match line.trim().strip_prefix("AETHERDESK_CLEANUP_ERROR_V1=")? {
                    "storage_full" => "The host disk or account quota is full",
                    "permission_denied" => {
                        "The host denied access to its AetherDesk maintenance files"
                    }
                    "filesystem_error" => "A host filesystem operation failed",
                    "unsafe_state" => {
                        "Host maintenance refused unsafe or incompatible AetherDesk files"
                    }
                    _ => return None,
                },
            )
        });
        let message = if let Some(reason) = reason {
            reason.to_owned()
        } else if self.status == Some(127) {
            "The host could not find python3; check its non-interactive SSH command environment"
                .into()
        } else {
            format!("Host maintenance ended before becoming ready (exit status {}). Check the host's SSH command policy and Python 3 environment",
                self.status.map_or_else(|| "unavailable".into(), |status| status.to_string()))
        };
        CoreError::ConnectionError(message)
    }
}

fn command(mode: &str, identity: ConnectionId, version: &str) -> CoreResult<String> {
    if version.is_empty() || !version.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return Err(CoreError::ConnectionError(
            "Invalid maintenance version".into(),
        ));
    }
    Ok(format!(
        "exec python3 -c {} {} {} {}",
        crate::startup::quote(include_str!("host_cleanup.py")),
        crate::startup::quote(mode),
        crate::startup::quote(&identity.as_uuid().to_string()),
        crate::startup::quote(version)
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn owned_host_residue_is_cleaned_without_touching_other_tabs_desktops_or_transfers() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("host_cleanup.py");
        std::fs::write(&script, include_str!("host_cleanup.py")).unwrap();
        let output = std::process::Command::new("python3")
            .args([
                "-c",
                include_str!("../../../../../tools/rust-build-kit/test_host_cleanup.py"),
            ])
            .env("AETHERDESK_HOST_CLEANUP_SCRIPT", &script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl SshSession {
    pub(crate) fn temporary_root(&self) -> Option<String> {
        self.temporary_root.lock().clone()
    }

    /// Optional maintenance. A lease channel's EOF cleans only this connection's
    /// owned files; other tabs and disconnected desktops hold separate state.
    pub async fn start_host_cleanup(&self, version: &str) -> CoreResult<()> {
        let operation = async {
            if self.temporary_root().is_some() {
                return Ok(());
            }
            let mut channel = OwnedChannel(Some(self.session.channel_open_session().await?));
            channel
                .exec(true, command("lease", self.connection_id, version)?)
                .await?;
            let mut response = SetupReply::default();
            let path = loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Data { data }) => {
                        if let Some(path) = response.ready(&data)? {
                            break path;
                        }
                    }
                    Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                        let remaining = 4096usize.saturating_sub(response.stderr.len());
                        response
                            .stderr
                            .extend_from_slice(&data[..data.len().min(remaining)]);
                    }
                    Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                        response.status = Some(exit_status);
                    }
                    // EOF can precede the final exit status. Wait for Close.
                    Some(russh::ChannelMsg::Close) | None => return Err(response.diagnostic()),
                    _ => {}
                }
            };
            if !path.starts_with('/')
                || !path.ends_with(&format!(
                    "/.aetherdesk/connections/{}",
                    self.connection_id.as_uuid()
                ))
                || path.chars().any(char::is_control)
            {
                return Err(CoreError::ConnectionError(
                    "Invalid private maintenance path".into(),
                ));
            }
            *self.temporary_root.lock() = Some(path);
            let temporary_root = self.temporary_root.clone();
            let cancel = self.lifetime.0.clone();
            let task = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            let _ = timeout(Duration::from_secs(1),channel.eof()).await;
                            let _ = timeout(Duration::from_secs(1),async {while channel.wait().await.is_some() {}}).await;
                            break;
                        }
                        msg = channel.wait() => if matches!(msg, None | Some(russh::ChannelMsg::Eof | russh::ChannelMsg::Close)) { break; },
                    }
                }
                *temporary_root.lock() = None;
            });
            *self.cleanup_task.lock().await = Some(task);
            Ok(())
        };
        timeout(Duration::from_secs(8), operation)
            .await
            .map_err(|_| CoreError::ConnectionError("Host maintenance setup timed out".into()))?
    }

    pub async fn mark_host_cache(&self, version: &str) -> CoreResult<()> {
        let output = self
            .execute_command_output(
                &command("mark-cache", self.connection_id, version)?,
                Duration::from_secs(3),
            )
            .await?;
        if output.exit_status != 0 {
            return Err(CoreError::ConnectionError(
                "Agent cache maintenance unavailable".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;
    #[test]
    fn maintenance_errors_distinguish_full_disk_permissions_and_missing_interpreter() {
        for (code, expected) in [
            ("storage_full", "quota"),
            ("permission_denied", "denied"),
            ("unsafe_state", "unsafe"),
            ("filesystem_error", "filesystem"),
        ] {
            let reply = SetupReply {
                stderr: format!("AETHERDESK_CLEANUP_ERROR_V1={code}\n").into_bytes(),
                status: Some(1),
                ..Default::default()
            };
            assert!(reply.diagnostic().to_string().contains(expected));
        }
        let mut reply = SetupReply {
            stderr: b"private path and environment".to_vec(),
            status: Some(1),
            ..Default::default()
        };
        assert!(!reply.diagnostic().to_string().contains("private path"));
        assert!(!reply.diagnostic().to_string().contains("could not find"));
        reply.status = Some(127);
        assert!(reply
            .diagnostic()
            .to_string()
            .contains("could not find python3"));
        reply.status = None;
        assert!(reply.diagnostic().to_string().contains("unavailable"));
    }
    #[test]
    fn maintenance_readiness_accepts_split_packets_and_rejects_unbounded_output() {
        let mut reply = SetupReply::default();
        assert!(reply.ready(b"AETHERDESK_CLEANUP_RE").unwrap().is_none());
        assert_eq!(
            reply.ready(b"ADY_V1=/private/path\n").unwrap(),
            Some("/private/path".into())
        );
        assert!(SetupReply::default().ready(&vec![b'x'; 4097]).is_err());
        assert!(SetupReply::default()
            .ready(b"unexpected shell banner\n")
            .is_err());
    }
}
