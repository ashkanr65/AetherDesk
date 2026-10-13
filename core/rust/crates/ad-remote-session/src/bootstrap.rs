use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncReadExt;

const MAX_AGENT_BYTES: u64 = 512 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const RUNTIME_MANIFEST_NAME: &str = "runtime-manifest.json";
const MAX_RUNTIME_FILES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCommandResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: u32,
}

impl RemoteCommandResult {
    pub fn success(&self) -> bool {
        self.exit_status == 0
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_owned()
    }
}

#[async_trait]
pub trait RemoteAgentHost: Send + Sync {
    async fn command(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<RemoteCommandResult, AgentBootstrapError>;

    async fn upload_resumable(
        &self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<(), AgentBootstrapError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentArtifact {
    pub version: String,
    pub local_path: PathBuf,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeManifest {
    pub schema_version: u16,
    pub files: Vec<RuntimeLibrary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLibrary {
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentRuntimeBundle {
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: RuntimeManifest,
}

impl AgentRuntimeBundle {
    pub async fn inspect(directory: impl Into<PathBuf>) -> Result<Self, AgentBootstrapError> {
        let directory = directory.into();
        let manifest_path = directory.join(RUNTIME_MANIFEST_NAME);
        let bytes = tokio::fs::read(&manifest_path).await?;
        if bytes.len() > 1024 * 1024 {
            return Err(AgentBootstrapError::InvalidRuntimeManifest);
        }
        let manifest: RuntimeManifest = serde_json::from_slice(&bytes)
            .map_err(|_| AgentBootstrapError::InvalidRuntimeManifest)?;
        validate_manifest(&directory, &manifest).await?;
        Ok(Self {
            directory,
            manifest_path,
            manifest,
        })
    }
}

impl AgentArtifact {
    pub async fn inspect(
        version: impl Into<String>,
        local_path: impl Into<PathBuf>,
    ) -> Result<Self, AgentBootstrapError> {
        let version = version.into();
        validate_component(&version)?;
        let local_path = local_path.into();
        let metadata = tokio::fs::metadata(&local_path).await?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_AGENT_BYTES {
            return Err(AgentBootstrapError::InvalidArtifact);
        }
        let mut file = tokio::fs::File::open(&local_path).await?;
        let mut digest = Context::new(&SHA256);
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let sha256 = digest
            .finish()
            .as_ref()
            .try_into()
            .map_err(|_| AgentBootstrapError::Digest)?;
        Ok(Self {
            version,
            local_path,
            size: metadata.len(),
            sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentDeployment {
    pub version: String,
    /// SFTP path relative to the authenticated user's home directory.
    pub remote_path: String,
    pub sha256: [u8; 32],
    pub uploaded: bool,
    pub bundled_runtime: bool,
}

pub struct AgentBootstrap<H: RemoteAgentHost> {
    host: Arc<H>,
}

impl<H: RemoteAgentHost> AgentBootstrap<H> {
    pub fn new(host: Arc<H>) -> Self {
        Self { host }
    }

    pub async fn ensure_deployed(
        &self,
        artifact: &AgentArtifact,
    ) -> Result<AgentDeployment, AgentBootstrapError> {
        self.ensure_bundle_deployed(artifact, None).await
    }

    pub async fn ensure_bundle_deployed(
        &self,
        artifact: &AgentArtifact,
        runtime: Option<&AgentRuntimeBundle>,
    ) -> Result<AgentDeployment, AgentBootstrapError> {
        validate_component(&artifact.version)?;
        let directory = format!(".aetherdesk/agent/{}", artifact.version);
        let final_path = format!("{directory}/ad-agent");
        let digest_hex = hex_encode(&artifact.sha256);
        let temporary_path = format!("{directory}/.ad-agent.{digest_hex}.part");
        let command_path = format!("$HOME/{final_path}");

        let probe = format!(
            "if [ -f {path} ] && [ -x {path} ]; then sha256sum -- {path} | cut -d' ' -f1; fi",
            path = shell_quote_expand_home(&command_path)?
        );
        let current = self.host.command(&probe, COMMAND_TIMEOUT).await?;
        if current.success() && current.stdout_text().eq_ignore_ascii_case(&digest_hex) {
            if let Some(runtime) = runtime {
                self.ensure_runtime_deployed(&directory, runtime).await?;
            }
            self.verify_runtime(&command_path, &artifact.version)
                .await?;
            return Ok(AgentDeployment {
                version: artifact.version.clone(),
                remote_path: final_path,
                sha256: artifact.sha256,
                uploaded: false,
                bundled_runtime: runtime.is_some(),
            });
        }

        let mkdir = format!(
            "umask 077 && mkdir -p -- {}",
            shell_quote_expand_home(&format!("$HOME/{directory}"))?
        );
        ensure_success(
            self.host.command(&mkdir, COMMAND_TIMEOUT).await?,
            "create agent directory",
        )?;

        // The digest-specific partial name makes retries idempotent. The SFTP
        // implementation resumes at the existing size after a network drop.
        self.host
            .upload_resumable(&artifact.local_path, &temporary_path)
            .await?;

        let temporary_command_path = format!("$HOME/{temporary_path}");
        let install = format!(
            "actual=$(sha256sum -- {tmp} | cut -d' ' -f1) && [ \"$actual\" = {digest} ] && chmod 700 -- {tmp} && mv -f -- {tmp} {final_path}",
            tmp = shell_quote_expand_home(&temporary_command_path)?,
            digest = shell_quote(&digest_hex)?,
            final_path = shell_quote_expand_home(&command_path)?,
        );
        let result = self.host.command(&install, COMMAND_TIMEOUT).await?;
        ensure_success(result, "verify and install agent")?;
        if let Some(runtime) = runtime {
            self.ensure_runtime_deployed(&directory, runtime).await?;
        }
        self.verify_runtime(&command_path, &artifact.version)
            .await?;

        Ok(AgentDeployment {
            version: artifact.version.clone(),
            remote_path: final_path,
            sha256: artifact.sha256,
            uploaded: true,
            bundled_runtime: runtime.is_some(),
        })
    }

    async fn ensure_runtime_deployed(
        &self,
        directory: &str,
        runtime: &AgentRuntimeBundle,
    ) -> Result<(), AgentBootstrapError> {
        let remote_libs = format!("$HOME/{directory}/libs");
        let mkdir = format!(
            "umask 077 && mkdir -p -- {}",
            shell_quote_expand_home(&remote_libs)?
        );
        ensure_success(
            self.host.command(&mkdir, COMMAND_TIMEOUT).await?,
            "create bundled runtime directory",
        )?;

        for library in &runtime.manifest.files {
            let remote = format!("{directory}/libs/{}", library.file_name);
            let remote_command = format!("$HOME/{remote}");
            let expected = library.sha256.to_ascii_lowercase();
            let probe = format!(
                "if [ -f {path} ]; then sha256sum -- {path} | cut -d' ' -f1; fi",
                path = shell_quote_expand_home(&remote_command)?
            );
            let current = self.host.command(&probe, COMMAND_TIMEOUT).await?;
            if current.success() && current.stdout_text().eq_ignore_ascii_case(&expected) {
                continue;
            }
            let partial = format!("{remote}.{}.part", &expected[..16]);
            self.host
                .upload_resumable(
                    &runtime.directory.join("libs").join(&library.file_name),
                    &partial,
                )
                .await?;
            let partial_command = format!("$HOME/{partial}");
            let install = format!(
                "actual=$(sha256sum -- {tmp} | cut -d' ' -f1) && [ \"$actual\" = {digest} ] && chmod 500 -- {tmp} && mv -f -- {tmp} {final_path}",
                tmp = shell_quote_expand_home(&partial_command)?,
                digest = shell_quote(&expected)?,
                final_path = shell_quote_expand_home(&remote_command)?,
            );
            ensure_success(
                self.host.command(&install, COMMAND_TIMEOUT).await?,
                "verify and install runtime library",
            )?;
        }

        let remote_manifest = format!("{directory}/{RUNTIME_MANIFEST_NAME}");
        self.host
            .upload_resumable(&runtime.manifest_path, &remote_manifest)
            .await?;
        Ok(())
    }

    async fn verify_runtime(
        &self,
        command_path: &str,
        version: &str,
    ) -> Result<(), AgentBootstrapError> {
        let directory = command_path
            .strip_suffix("/ad-agent")
            .ok_or(AgentBootstrapError::UnsafePath)?;
        let libs = format!("{directory}/libs");
        let command = format!(
            "PATH={libs}${{PATH:+:$PATH}} LD_LIBRARY_PATH={libs}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}} GST_PLUGIN_PATH={libs} GST_PLUGIN_SYSTEM_PATH_1_0={libs} GST_PLUGIN_SCANNER={libs}/gst-plugin-scanner {agent} --version",
            libs = shell_quote_expand_home(&libs)?,
            agent = shell_quote_expand_home(command_path)?,
        );
        let result = self.host.command(&command, COMMAND_TIMEOUT).await?;
        ensure_success(result.clone(), "start uploaded agent")?;
        let expected = format!("ad-agent {version}");
        if result.stdout_text() != expected {
            return Err(AgentBootstrapError::RuntimeValidation {
                expected,
                actual: redact_and_bound(&result.stdout_text(), 256),
            });
        }
        Ok(())
    }
}

async fn validate_manifest(
    directory: &Path,
    manifest: &RuntimeManifest,
) -> Result<(), AgentBootstrapError> {
    if manifest.schema_version != 1
        || manifest.files.is_empty()
        || manifest.files.len() > MAX_RUNTIME_FILES
    {
        return Err(AgentBootstrapError::InvalidRuntimeManifest);
    }
    let mut names = std::collections::HashSet::new();
    for library in &manifest.files {
        validate_component(&library.file_name)?;
        if library.size == 0
            || library.size > MAX_AGENT_BYTES
            || library.sha256.len() != 64
            || !library.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !names.insert(library.file_name.clone())
        {
            return Err(AgentBootstrapError::InvalidRuntimeManifest);
        }
        let path = directory.join("libs").join(&library.file_name);
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() || metadata.len() != library.size {
            return Err(AgentBootstrapError::RuntimeLibraryMismatch(
                library.file_name.clone(),
            ));
        }
        let actual = digest_file(&path).await?;
        if !hex_encode(&actual).eq_ignore_ascii_case(&library.sha256) {
            return Err(AgentBootstrapError::RuntimeLibraryMismatch(
                library.file_name.clone(),
            ));
        }
    }
    Ok(())
}

async fn digest_file(path: &Path) -> Result<[u8; 32], AgentBootstrapError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut digest = Context::new(&SHA256);
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    digest
        .finish()
        .as_ref()
        .try_into()
        .map_err(|_| AgentBootstrapError::Digest)
}

fn ensure_success(
    result: RemoteCommandResult,
    operation: &'static str,
) -> Result<(), AgentBootstrapError> {
    if result.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&result.stderr);
    Err(AgentBootstrapError::RemoteCommand {
        operation,
        exit_status: result.exit_status,
        detail: redact_and_bound(&stderr, 2048),
    })
}

fn validate_component(component: &str) -> Result<(), AgentBootstrapError> {
    if component.is_empty()
        || component.len() > 64
        || !component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AgentBootstrapError::InvalidVersion);
    }
    Ok(())
}

fn shell_quote(value: &str) -> Result<String, AgentBootstrapError> {
    if value.contains('\0') || value.contains('\n') || value.contains('\r') {
        return Err(AgentBootstrapError::UnsafePath);
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// Quote a fixed `$HOME/` path while retaining only the intended environment
/// expansion. Every remaining byte is single-quoted.
fn shell_quote_expand_home(value: &str) -> Result<String, AgentBootstrapError> {
    let suffix = value
        .strip_prefix("$HOME/")
        .ok_or(AgentBootstrapError::UnsafePath)?;
    Ok(format!("\"$HOME\"/{}", shell_quote(suffix)?))
}

pub(crate) fn hex_encode(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(value.len() * 2);
    for byte in value {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn redact_and_bound(value: &str, maximum: usize) -> String {
    let bounded: String = value.chars().take(maximum).collect();
    bounded
        .replace("password", "[redacted]")
        .replace("token", "[redacted]")
}

#[derive(Debug, Error)]
pub enum AgentBootstrapError {
    #[error("agent version is not a safe path component")]
    InvalidVersion,
    #[error("agent artifact must be a regular file between 1 byte and 512 MiB")]
    InvalidArtifact,
    #[error("agent path contains unsafe characters")]
    UnsafePath,
    #[error("failed to calculate the agent digest")]
    Digest,
    #[error("agent file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("remote operation `{operation}` failed with status {exit_status}: {detail}")]
    RemoteCommand {
        operation: &'static str,
        exit_status: u32,
        detail: String,
    },
    #[error("SSH operation failed: {0}")]
    Ssh(String),
    #[error("uploaded agent runtime check failed: expected `{expected}`, received `{actual}`")]
    RuntimeValidation { expected: String, actual: String },
    #[error("runtime manifest is invalid")]
    InvalidRuntimeManifest,
    #[error("bundled runtime library `{0}` does not match its manifest")]
    RuntimeLibraryMismatch(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryHost {
        files: Mutex<HashMap<String, Vec<u8>>>,
        commands: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl RemoteAgentHost for MemoryHost {
        async fn command(
            &self,
            command: &str,
            _timeout: Duration,
        ) -> Result<RemoteCommandResult, AgentBootstrapError> {
            self.commands.lock().unwrap().push(command.to_owned());
            let stdout = if command.ends_with(" --version") {
                b"ad-agent 1.2.3\n".to_vec()
            } else {
                Vec::new()
            };
            Ok(RemoteCommandResult {
                stdout,
                stderr: Vec::new(),
                exit_status: 0,
            })
        }

        async fn upload_resumable(
            &self,
            local_path: &Path,
            remote_path: &str,
        ) -> Result<(), AgentBootstrapError> {
            let data = tokio::fs::read(local_path).await?;
            self.files
                .lock()
                .unwrap()
                .insert(remote_path.to_owned(), data);
            Ok(())
        }
    }

    #[tokio::test]
    async fn deploys_to_a_digest_named_partial_then_installs_atomically() {
        let local = std::env::temp_dir().join(format!("ad-agent-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&local, b"agent").await.unwrap();
        let artifact = AgentArtifact::inspect("1.2.3", &local).await.unwrap();
        let host = Arc::new(MemoryHost::default());
        let deployed = AgentBootstrap::new(host.clone())
            .ensure_deployed(&artifact)
            .await
            .unwrap();
        assert!(deployed.uploaded);
        {
            let files = host.files.lock().unwrap();
            let path = files.keys().next().unwrap();
            assert!(path.starts_with(".aetherdesk/agent/1.2.3/.ad-agent."));
            assert!(path.ends_with(".part"));
        }
        assert!(host
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|command| command.contains("chmod 700") && command.contains("mv -f")));
        let _ = tokio::fs::remove_file(local).await;
    }

    #[tokio::test]
    async fn rejects_path_traversal_in_versions() {
        let result = AgentArtifact::inspect("../latest", "missing").await;
        assert!(matches!(result, Err(AgentBootstrapError::InvalidVersion)));
    }

    #[tokio::test]
    async fn validates_every_file_in_a_bundled_runtime_manifest() {
        let root = std::env::temp_dir().join(format!("ad-runtime-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("libs")).await.unwrap();
        let library = root.join("libs/libaether-test.so.1");
        tokio::fs::write(&library, b"runtime library")
            .await
            .unwrap();
        let digest = digest_file(&library).await.unwrap();
        let manifest = RuntimeManifest {
            schema_version: 1,
            files: vec![RuntimeLibrary {
                file_name: "libaether-test.so.1".into(),
                size: 15,
                sha256: hex_encode(&digest),
            }],
        };
        tokio::fs::write(
            root.join(RUNTIME_MANIFEST_NAME),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let inspected = AgentRuntimeBundle::inspect(&root).await.unwrap();
        assert_eq!(inspected.manifest, manifest);

        tokio::fs::write(&library, b"tampered runtime")
            .await
            .unwrap();
        assert!(matches!(
            AgentRuntimeBundle::inspect(&root).await,
            Err(AgentBootstrapError::RuntimeLibraryMismatch(_))
        ));
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
