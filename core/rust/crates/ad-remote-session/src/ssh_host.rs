use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ad_ssh::SshSession;
use ad_stream_client::{SshChannelIo, SshTransportAdapter, TransportAdapter};
use ad_stream_protocol::{SessionParameters, VideoCodec};
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::bootstrap::{
    AgentBootstrapError, AgentDeployment, RemoteAgentHost, RemoteCommandResult,
};
use crate::lifecycle::{channel_prelude, AgentConnection, AgentEndpoint, LifecycleError};

// Includes a serialized launch, confirmed-dead legacy recovery and fresh startup.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
const ENDPOINT_PREFIX: &str = "AETHERDESK_AGENT_V1";

pub struct SshAgentHost {
    session: Arc<SshSession>,
    startup: Option<ad_ssh::StartupScript>,
}

impl SshAgentHost {
    pub fn new(session: Arc<SshSession>) -> Self {
        Self {
            session,
            startup: None,
        }
    }
    pub fn with_startup(mut self, startup: Option<ad_ssh::StartupScript>) -> Self {
        self.startup = startup;
        self
    }

    pub fn ssh_session(&self) -> &Arc<SshSession> {
        &self.session
    }
}

#[async_trait]
impl RemoteAgentHost for SshAgentHost {
    async fn command(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<RemoteCommandResult, AgentBootstrapError> {
        let output = self
            .session
            .execute_command_output(command, timeout)
            .await
            .map_err(|error| AgentBootstrapError::Ssh(error.to_string()))?;
        Ok(RemoteCommandResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_status: output.exit_status,
        })
    }

    async fn upload_resumable(
        &self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<(), AgentBootstrapError> {
        let local = local_path
            .to_str()
            .ok_or_else(|| AgentBootstrapError::Ssh("agent path is not Unicode".into()))?;
        self.session
            .sftp_upload_resumable(local, remote_path, true)
            .await
            .map_err(|error| AgentBootstrapError::Ssh(error.to_string()))
    }
}

#[async_trait]
impl AgentConnection for SshAgentHost {
    fn launch_timeout(&self) -> Duration {
        self.startup.as_ref().map_or(Duration::from_secs(65), |s| {
            LAUNCH_TIMEOUT + s.timeout() + Duration::from_secs(5)
        })
    }
    async fn session_ended(&self, session_id: Uuid) -> Result<bool, LifecycleError> {
        // Old agents remove this directory only after shutting down their
        // compositor. Read-only verification preserves already-running sessions.
        let command = format!(
            "test -n \"$HOME\" && test -d \"$HOME/.aetherdesk/sessions\" && test -r \"$HOME/.aetherdesk/sessions\" && test -x \"$HOME/.aetherdesk/sessions\" && test ! -e \"$HOME/.aetherdesk/sessions/{session_id}\""
        );
        Ok(self
            .command(&command, Duration::from_secs(3))
            .await?
            .success())
    }

    async fn launch_agent(
        &self,
        deployment: &AgentDeployment,
        session_id: Uuid,
        parameters: Option<&SessionParameters>,
    ) -> Result<AgentEndpoint, LifecycleError> {
        let remote_path = safe_remote_path(&deployment.remote_path)?;
        let session_options = parameters
            .map(agent_options)
            .transpose()?
            .unwrap_or_default();
        let executable = format!(
            "\"$HOME\"/{remote_path}",
            remote_path = shell_quote(remote_path)
        );
        let command = if deployment.bundled_runtime {
            let directory = remote_path
                .strip_suffix("/ad-agent")
                .ok_or(LifecycleError::InvalidEndpoint)?;
            format!(
                "PATH=\"$HOME\"/{libs}${{PATH:+:$PATH}} LD_LIBRARY_PATH=\"$HOME\"/{libs}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}} GST_PLUGIN_PATH=\"$HOME\"/{libs} GST_PLUGIN_SYSTEM_PATH_1_0=\"$HOME\"/{libs} GST_PLUGIN_SCANNER=\"$HOME\"/{libs}/gst-plugin-scanner {executable} --launch --session-id {session_id}{session_options}",
                libs = shell_quote(&format!("{directory}/libs")),
            )
        } else {
            format!("{executable} --launch --session-id {session_id}{session_options}")
        };
        let command = if let Some(startup) = &self.startup {
            format!(
                "{}{}",
                startup
                    .environment_prefix()
                    .map_err(LifecycleError::Launch)?,
                command
            )
        } else {
            command
        };
        let launch_timeout = self
            .startup
            .as_ref()
            .map_or(LAUNCH_TIMEOUT, |s| LAUNCH_TIMEOUT + s.timeout());
        let output = self
            .session
            .execute_command_output(&command, launch_timeout)
            .await
            .map_err(|error| LifecycleError::Launch(error.to_string()))?;
        if output.exit_status != 0 {
            return Err(LifecycleError::Launch(format!(
                "agent launcher exited with status {}: {}",
                output.exit_status,
                bounded_text(&output.stderr, 2048)
            )));
        }
        parse_endpoint(&output.stdout)
    }

    async fn open_transport(
        &self,
        endpoint: &AgentEndpoint,
    ) -> Result<Arc<dyn TransportAdapter>, LifecycleError> {
        let control = self.open_channel(endpoint, 0).await?;
        let input = self.open_channel(endpoint, 1).await?;
        let media = self.open_channel(endpoint, 2).await?;
        Ok(Arc::new(SshTransportAdapter::new(control, input, media)))
    }
}

fn agent_options(parameters: &SessionParameters) -> Result<String, LifecycleError> {
    let desktop = safe_identifier(&parameters.desktop_environment)?;
    let codec = match parameters.video_codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Av1 => "av1",
        _ => return Err(LifecycleError::UnsupportedParameters),
    };
    Ok(format!(
        " --desktop {} --width {} --height {} --fps {} --bitrate-kbps {} --codec {codec}",
        shell_quote(desktop),
        parameters.width,
        parameters.height,
        parameters.target_fps,
        parameters.target_bitrate_kbps,
    ))
}

fn safe_identifier(value: &str) -> Result<&str, LifecycleError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(LifecycleError::InvalidParameters);
    }
    Ok(value)
}

impl SshAgentHost {
    async fn open_channel(
        &self,
        endpoint: &AgentEndpoint,
        channel_id: u8,
    ) -> Result<SshChannelIo, LifecycleError> {
        let channel = if channel_id == 2 {
            self.session.open_bulk_loopback_channel(endpoint.port).await
        } else {
            self.session
                .open_remote_loopback_channel(endpoint.port)
                .await
        }
        .map_err(|error| LifecycleError::Launch(error.to_string()))?;
        let mut stream = channel.into_stream();
        stream
            .write_all(&channel_prelude(channel_id, &endpoint.token)?)
            .await
            .map_err(|error| LifecycleError::Launch(error.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|error| LifecycleError::Launch(error.to_string()))?;
        Ok(SshChannelIo::from_stream(stream))
    }
}

fn parse_endpoint(output: &[u8]) -> Result<AgentEndpoint, LifecycleError> {
    if output.len() > 4096 {
        return Err(LifecycleError::InvalidEndpoint);
    }
    let text = std::str::from_utf8(output).map_err(|_| LifecycleError::InvalidEndpoint)?;
    let line = text
        .lines()
        .find(|line| line.starts_with(ENDPOINT_PREFIX))
        .ok_or(LifecycleError::InvalidEndpoint)?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some(ENDPOINT_PREFIX) {
        return Err(LifecycleError::InvalidEndpoint);
    }
    let port = fields
        .next()
        .ok_or(LifecycleError::InvalidEndpoint)?
        .parse::<u16>()
        .map_err(|_| LifecycleError::InvalidEndpoint)?;
    let token = decode_token(fields.next().ok_or(LifecycleError::InvalidEndpoint)?)?;
    if fields.next().is_some() || port == 0 {
        return Err(LifecycleError::InvalidEndpoint);
    }
    Ok(AgentEndpoint {
        port,
        token,
        newly_created: text
            .lines()
            .any(|line| line == "AETHERDESK_AGENT_CREATED_V1"),
    })
}

fn decode_token(value: &str) -> Result<[u8; 32], LifecycleError> {
    if value.len() != 64 {
        return Err(LifecycleError::InvalidEndpoint);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    if output.iter().all(|byte| *byte == 0) {
        return Err(LifecycleError::InvalidEndpoint);
    }
    Ok(output)
}

fn hex_nibble(value: u8) -> Result<u8, LifecycleError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(LifecycleError::InvalidEndpoint),
    }
}

fn safe_remote_path(path: &str) -> Result<&str, LifecycleError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\0')
        || path.contains('\n')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(LifecycleError::InvalidEndpoint);
    }
    Ok(path)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn bounded_text(value: &[u8], maximum: usize) -> String {
    String::from_utf8_lossy(value)
        .chars()
        .take(maximum)
        .collect::<String>()
        .replace("token", "[redacted]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_endpoint_without_logging_the_token() {
        let token = "01".repeat(32);
        let endpoint =
            parse_endpoint(format!("AETHERDESK_AGENT_V1 41234 {token}\n").as_bytes()).unwrap();
        assert_eq!(endpoint.port, 41234);
        assert_eq!(endpoint.token, [1; 32]);
        assert!(!endpoint.newly_created);
        let fresh = parse_endpoint(
            format!("AETHERDESK_AGENT_CREATED_V1\nAETHERDESK_AGENT_V1 41234 {token}\n").as_bytes(),
        )
        .unwrap();
        assert!(fresh.newly_created);
    }

    #[test]
    fn rejects_traversal_in_deployment_paths() {
        assert!(safe_remote_path(".aetherdesk/../ad-agent").is_err());
    }
}
