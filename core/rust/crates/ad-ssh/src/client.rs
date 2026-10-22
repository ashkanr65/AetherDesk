//! SSH client implementation with real SSH connections

#[cfg(test)]
#[path = "isolation_tests.rs"]
mod isolation_tests;

pub use ad_config::{SshConfig, X11ForwardingConfig};
#[path = "remote_file.rs"]
mod remote_file;
#[path = "resumable_file.rs"]
mod resumable_file;
pub use remote_file::{FileProgress, RemoteFileMetadata, RemoteFileRequest, RemoteFileResult};
pub use resumable_file::{rename_local_noclobber, ResumableTransferFile};
#[path = "language_server.rs"]
mod language_server;
pub use language_server::{LanguageServer, LanguageServerConfig};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub key_fingerprint: String,
}

use ad_core::{ConnectionId, CoreError, Result as CoreResult};
use ad_events::EventBus;
use russh::client as ssh_client;
use russh::keys::{
    load_secret_key, Algorithm, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::future::Future;
use std::io::{Error as IoError, ErrorKind, SeekFrom};
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info, warn};

/// SSH Client for establishing connections
pub struct SshClient {
    config: SshConfig,
    event_bus: Option<Arc<EventBus>>,
    #[cfg(test)]
    pinned_test_key: Option<russh::keys::PublicKey>,
}

/// Metadata returned by the built-in SFTP v3 client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub modified: Option<u32>,
}

/// Complete result of a non-interactive SSH command.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: u32,
}

impl CommandOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

fn client_preferences(x11: bool) -> russh::Preferred {
    let mut preferred = russh::Preferred::default();
    preferred.key = std::borrow::Cow::Owned(
        preferred
            .key
            .iter()
            .filter(|algorithm| !matches!(algorithm, Algorithm::Rsa { hash: None }))
            .cloned()
            .collect(),
    );
    if x11 {
        // Raw X11 pixels/requests compress well; defer compression until authentication.
        preferred.compression = std::borrow::Cow::Owned(vec![
            russh::compression::ZLIB_LEGACY,
            russh::compression::NONE,
        ]);
    }
    preferred
}

impl SshClient {
    /// Create a new SSH client
    pub fn new(config: SshConfig, event_bus: Arc<EventBus>) -> Self {
        Self {
            config,
            event_bus: Some(event_bus),
            #[cfg(test)]
            pinned_test_key: None,
        }
    }

    /// Create a client with an internal event bus. This is convenient for FFI
    /// consumers that poll terminal output directly.
    pub fn standalone(config: SshConfig) -> Self {
        Self {
            config,
            event_bus: None,
            #[cfg(test)]
            pinned_test_key: None,
        }
    }

    /// Connect to the SSH server
    pub async fn connect(&self) -> CoreResult<SshSession> {
        if self.config.x11.is_some() {
            self.connect_with_bulk_transport().await
        } else {
            self.connect_single().await
        }
    }

    /// Graphics must not share Russh's bounded packet reader with interactive I/O.
    /// Both links authenticate independently and use the same configured VPN.
    pub async fn connect_with_bulk_transport(&self) -> CoreResult<SshSession> {
        let mut interactive_config = self.config.clone();
        interactive_config.x11 = None;
        let interactive = Self {
            config: interactive_config,
            event_bus: self.event_bus.clone(),
            #[cfg(test)]
            pinned_test_key: self.pinned_test_key.clone(),
        };
        let mut session = interactive.connect_single().await?;
        let bulk = match self.connect_single().await {
            Ok(bulk) => bulk,
            Err(error) => {
                session.close_single().await?;
                return Err(error);
            }
        };
        session.x11 = self.config.x11.clone();
        session.bulk = Some(Arc::new(bulk));
        Ok(session)
    }

    async fn connect_single(&self) -> CoreResult<SshSession> {
        info!(
            "Connecting to SSH server at {}:{}",
            self.config.host, self.config.port
        );

        // Create configuration for russh
        // Russh's safe defaults already exclude CBC, 3DES, SHA-1 MACs and
        // SHA-1 key exchange. Remove legacy ssh-rsa/SHA-1 host signatures as
        // well so a weak server fails closed instead of downgrading.
        let preferred = client_preferences(self.config.x11.is_some());
        let config = ssh_client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(60)),
            keepalive_interval: Some(std::time::Duration::from_secs(15)),
            keepalive_max: 3,
            nodelay: true,
            preferred,
            ..Default::default()
        };

        let lifetime = Arc::new(SessionLifetime(tokio_util::sync::CancellationToken::new()));
        let verified_key = Arc::new(parking_lot::Mutex::new(None));
        // Create handler
        let handler = ClientHandler {
            verified_key: verified_key.clone(),
            event_bus: self.event_bus.clone(),
            host: self.config.host.clone(),
            port: self.config.port,
            trust_new_host_key: self.config.trust_new_host_key,
            confirmed_host_fingerprint: self.config.confirmed_host_fingerprint.clone(),
            x11: self.config.x11.clone(),
            cancel: lifetime.0.clone(),
            #[cfg(test)]
            pinned_test_key: self.pinned_test_key.clone(),
        };

        // Connect to the server
        let connect_timeout = Duration::from_secs(self.config.timeout_secs.max(1));
        let mut session = match timeout(connect_timeout, async {
            let stream: Box<dyn ConnectionStream> =
                if let Some(path) = &self.config.wireguard_config_path {
                    Box::new(
                        ad_vpn::connect(
                            path,
                            &self.config.host,
                            self.config.port,
                            lifetime.0.clone(),
                        )
                        .await?,
                    )
                } else {
                    let stream =
                        TcpStream::connect((self.config.host.as_str(), self.config.port)).await?;
                    stream.set_nodelay(true)?;
                    Box::new(stream)
                };
            ssh_client::connect_stream(Arc::new(config), stream, handler).await
        })
        .await
        {
            Ok(Ok(session)) => {
                info!("Connected to SSH server");
                session
            }
            Ok(Err(e)) => {
                error!("Failed to connect to SSH server: {}", e);
                return Err(CoreError::ConnectionError(format!(
                    "SSH connection failed: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(CoreError::ConnectionError(format!(
                    "SSH connection to {}:{} timed out after {} seconds",
                    self.config.host, self.config.port, self.config.timeout_secs
                )));
            }
        };

        // Perform authentication
        let username = self.config.username.clone();

        if let Some(ref password) = self.config.password {
            info!("Attempting password authentication for user: {}", username);
            match session
                .authenticate_password(username.clone(), password.clone())
                .await
            {
                Ok(result) if result.success() => info!("Password authentication succeeded"),
                Ok(_) => {
                    error!("Password authentication rejected by server");
                    return Err(CoreError::ConnectionError(
                        "Authentication failed".to_string(),
                    ));
                }
                Err(e) => {
                    error!("Password authentication error: {}", e);
                    return Err(CoreError::ConnectionError(format!("Auth error: {}", e)));
                }
            }
        } else if let Some(ref key_path) = self.config.key_path {
            info!(
                "Attempting public key authentication with key: {:?}",
                key_path
            );
            match load_secret_key(key_path, self.config.key_passphrase.as_deref()) {
                Ok(key_pair) => {
                    let hash_algorithm = match session.best_supported_rsa_hash().await {
                        Ok(hash_algorithm) => hash_algorithm.flatten(),
                        Err(e) => {
                            error!("Failed to negotiate an RSA hash algorithm: {}", e);
                            return Err(CoreError::ConnectionError(format!(
                                "RSA hash negotiation failed: {}",
                                e
                            )));
                        }
                    };
                    let private_key =
                        PrivateKeyWithHashAlg::new(Arc::new(key_pair), hash_algorithm);

                    match session
                        .authenticate_publickey(username.clone(), private_key)
                        .await
                    {
                        Ok(result) if result.success() => {
                            info!("Public key authentication succeeded")
                        }
                        Ok(_) => {
                            error!("Public key authentication rejected by server");
                            return Err(CoreError::ConnectionError(
                                "Authentication failed".to_string(),
                            ));
                        }
                        Err(e) => {
                            error!("Public key authentication error: {}", e);
                            return Err(CoreError::ConnectionError(format!("Auth error: {}", e)));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to load private key: {}", e);
                    return Err(CoreError::ConnectionError(format!("Key load error: {}", e)));
                }
            }
        } else {
            // Try none authentication
            info!("No credentials provided, attempting none authentication");
            match session.authenticate_none(username.clone()).await {
                Ok(result) if result.success() => info!("None authentication succeeded"),
                Ok(_) => {
                    warn!("None authentication rejected, server requires credentials");
                    return Err(CoreError::ConnectionError(
                        "Authentication required".to_string(),
                    ));
                }
                Err(e) => {
                    error!("None authentication error: {}", e);
                    return Err(CoreError::ConnectionError(format!("Auth error: {}", e)));
                }
            }
        }

        let ssh_session = SshSession::new(
            ConnectionId::new(),
            session,
            self.config.x11.clone(),
            lifetime,
            HostIdentity {
                host: self.config.host.clone(),
                port: self.config.port,
                username: self.config.username.clone(),
                key_fingerprint: verified_key.lock().clone().ok_or_else(|| {
                    CoreError::ConnectionError("SSH host key was not verified".into())
                })?,
            },
        );

        Ok(ssh_session)
    }
}

/// Remove only this generated input and exact entries left by older versions.
/// A leading space alone is insufficient when Bash HISTCONTROL lacks
/// ignorespace. Keep the user's history settings and history file untouched;
/// deleting from highest to lowest preserves all unrelated entry numbers.
/// This runs before the first user command. Flush only the bootstrap's history
/// bookkeeping to /dev/null afterward: deleting previously loaded entries can
/// otherwise make Bash omit subsequent real commands when appending on exit.
fn private_shell_bootstrap(command: &str) -> String {
    let marker = format!("aetherdesk-bootstrap-{}", ConnectionId::new().as_uuid());
    // Embedding the complete initialization again inside its own history
    // matcher can exceed the PTY's canonical input limit before Readline starts.
    // Large integration lines are identified by their reserved function names.
    let legacy = if command.len() <= 256 {
        format!("{command}; stty echo")
    } else {
        String::new()
    }
    .replace('\'', "'\\''");
    let cleanup = format!(
        "__aetherdesk_clean_history() {{ \
         local __ad_n __ad_line __ad_i; local -a __ad_drop=(); \
         while IFS=$' \\t' read -r __ad_n __ad_line; do \
         __ad_n=${{__ad_n%\\*}}; \
         case \"$__ad_n\" in ''|*[!0-9]*) continue;; esac; \
         if [[ \"$__ad_line\" == *'{marker}'* || \"$__ad_line\" == '{legacy}' || \
         ( \"$__ad_line\" == *'aetherdesk-bootstrap-'* && \"$__ad_line\" == *'__aetherdesk_clean_history()'* ) || \
         \"$__ad_line\" == *'then __aetherdesk_emit_cwd()'* ]]; then \
         __ad_drop+=(\"$__ad_n\"); fi; \
         done < <(HISTTIMEFORMAT= builtin history); \
         for ((__ad_i=${{#__ad_drop[@]}}-1; __ad_i>=0; __ad_i--)); do \
         builtin history -d \"${{__ad_drop[__ad_i]}}\"; done; \
         builtin history -a /dev/null; \
         }}; __aetherdesk_clean_history; unset -f __aetherdesk_clean_history"
    );
    // Quote Bash-only syntax so other shells can still parse the bootstrap
    // and restore terminal echo, even though they do not use Bash history.
    let cleanup = cleanup.replace('\'', "'\\''");
    format!(" if [ -n \"${{BASH_VERSION-}}\" ]; then eval '{cleanup}'; fi; {command}; stty echo\r")
}

trait ConnectionStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> ConnectionStream for T {}

fn x11_display_environment(output: &[u8]) -> CoreResult<Option<String>> {
    let text = String::from_utf8_lossy(output);
    let Some((_, values)) = text.split_once("__AETHERDESK_X11__") else {
        return Ok(None);
    };
    let mut lines = values.splitn(3, '\n');
    let display = lines.next().unwrap_or_default();
    let Some(authority) = lines.next() else {
        return Ok(None);
    };
    if lines.next().is_none() {
        return Ok(None);
    }
    if !display.contains(':')
        || !display
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".:_-[]".contains(&b))
        || !authority.starts_with('/')
        || authority.contains(['\r', '\0'])
    {
        return Err(CoreError::ConnectionError(
            "The SSH server returned an invalid X11 environment".into(),
        ));
    }
    Ok(Some(format!(
        "export DISPLAY='{}' XAUTHORITY='{}'",
        display,
        authority.replace('\'', "'\\''")
    )))
}

struct OwnedChannel(Option<russh::Channel<ssh_client::Msg>>);
impl OwnedChannel {
    fn into_inner(mut self) -> russh::Channel<ssh_client::Msg> {
        self.0.take().expect("owned channel")
    }
}
impl std::ops::Deref for OwnedChannel {
    type Target = russh::Channel<ssh_client::Msg>;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("owned channel")
    }
}
impl std::ops::DerefMut for OwnedChannel {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("owned channel")
    }
}
impl Drop for OwnedChannel {
    fn drop(&mut self) {
        if let (Some(channel), Ok(runtime)) = (self.0.take(), tokio::runtime::Handle::try_current())
        {
            runtime.spawn(async move {
                let _ = timeout(Duration::from_secs(1), channel.eof()).await;
                let _ = timeout(Duration::from_secs(1), channel.close()).await;
            });
        }
    }
}

struct SessionLifetime(tokio_util::sync::CancellationToken);
impl Drop for SessionLifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Active SSH session
#[derive(Clone)]
pub struct SshSession {
    connection_id: ConnectionId,
    session: Arc<ssh_client::Handle<ClientHandler>>,
    channel: Arc<parking_lot::Mutex<Option<crate::shell::ShellIo>>>,
    x11: Option<X11ForwardingConfig>,
    forward_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    lifetime: Arc<SessionLifetime>,
    bulk: Option<Arc<SshSession>>,
    host_identity: HostIdentity,
}

impl SshSession {
    fn new(
        connection_id: ConnectionId,
        session: ssh_client::Handle<ClientHandler>,
        x11: Option<X11ForwardingConfig>,
        lifetime: Arc<SessionLifetime>,
        host_identity: HostIdentity,
    ) -> Self {
        Self {
            connection_id,
            session: Arc::new(session),
            channel: Arc::new(parking_lot::Mutex::new(None)),
            x11,
            forward_tasks: Arc::new(Mutex::new(Vec::new())),
            lifetime,
            bulk: None,
            host_identity,
        }
    }

    /// Get the connection ID
    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }
    pub fn host_identity(&self) -> HostIdentity {
        self.host_identity.clone()
    }

    /// Observe transport health without opening a shell or a probe process.
    pub fn is_closed(&self) -> bool {
        self.lifetime.0.is_cancelled()
            || self.session.is_closed()
            || self
                .bulk
                .as_ref()
                .is_some_and(|session| session.is_closed())
    }

    /// Open a shell channel for interactive terminal
    pub async fn open_shell(&mut self) -> CoreResult<()> {
        let shell_timeout = Duration::from_secs(15);
        timeout(shell_timeout, self.open_shell_inner(None))
            .await
            .map_err(|_| {
                CoreError::ConnectionError(
                    "The SSH server did not open a terminal within 15 seconds".to_string(),
                )
            })?
    }

    /// Open an interactive shell and submit one private bootstrap command
    /// before enabling terminal echo. This is used for shell integration that
    /// must not become visible terminal content or shell history.
    pub async fn open_shell_with_bootstrap(&mut self, bootstrap: &str) -> CoreResult<()> {
        if bootstrap.contains(['\r', '\n', '\0']) {
            return Err(CoreError::ConnectionError(
                "The shell bootstrap must be one command line".to_string(),
            ));
        }
        let shell_timeout = Duration::from_secs(15);
        timeout(shell_timeout, self.open_shell_inner(Some(bootstrap)))
            .await
            .map_err(|_| {
                CoreError::ConnectionError(
                    "The SSH server did not open a terminal within 15 seconds".to_string(),
                )
            })?
    }

    async fn open_shell_inner(&mut self, bootstrap: Option<&str>) -> CoreResult<()> {
        let display_environment = if self.x11.is_some() {
            Some(self.open_x11_anchor().await?)
        } else {
            None
        };
        let icon_helper = if self.x11.is_some() && bootstrap.is_some() {
            match self.prepare_x11_icons().await {
                Ok(value) => value,
                Err(error) => {
                    warn!(%error, "optional X11 icon helper unavailable");
                    None
                }
            }
        } else {
            None
        };
        match self.session.channel_open_session().await {
            Ok(channel) => {
                let channel = OwnedChannel(Some(channel));
                let private_command = match (display_environment, bootstrap) {
                    (Some(environment), command) => {
                        Some(format!("{environment}; {}", command.unwrap_or(":")))
                    }
                    (None, command) => command.map(str::to_owned),
                };
                let terminal_modes = private_command
                    .as_ref()
                    .map(|_| vec![(russh::Pty::ECHO, 0)])
                    .unwrap_or_default();
                channel
                    .request_pty(true, "xterm-256color", 80, 24, 0, 0, &terminal_modes)
                    .await?;
                channel.request_shell(true).await?;
                if let Some(command) = private_command {
                    let command = if let Some(helper) = &icon_helper {
                        format!("{command}; {helper}")
                    } else {
                        command.to_owned()
                    };
                    let private_input = private_shell_bootstrap(&command);
                    channel.data_bytes(private_input.into_bytes()).await?;
                }
                *self.channel.lock() = Some(crate::shell::ShellIo::spawn(
                    channel.into_inner(),
                    self.lifetime.0.clone(),
                ));
                info!("Shell channel opened");
                Ok(())
            }
            Err(e) => {
                error!("Failed to open shell channel: {}", e);
                Err(CoreError::ConnectionError(format!(
                    "Failed to open shell: {}",
                    e
                )))
            }
        }
    }

    async fn open_x11_anchor(&self) -> CoreResult<String> {
        let bulk = self.bulk.as_ref().ok_or_else(|| {
            CoreError::ConnectionError("X11 requires a dedicated graphics connection".into())
        })?;
        let x11 = self.x11.as_ref().expect("X11 configuration");
        let mut channel = OwnedChannel(Some(bulk.session.channel_open_session().await?));
        channel
            .request_x11(
                true,
                false,
                x11.authentication_protocol.clone(),
                x11.authentication_cookie.clone(),
                x11.screen_number,
            )
            .await?;
        // Keep the server's DISPLAY forwarding alive on the graphics link.
        // No credential is copied to the terminal; OpenSSH installs the cookie.
        channel.exec(true, "printf '__AETHERDESK_X11__%s\\n%s\\n' \"$DISPLAY\" \"${XAUTHORITY:-$HOME/.Xauthority}\"; exec cat >/dev/null").await?;
        let mut output = Vec::new();
        let environment = loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.extend_from_slice(&data);
                    if output.len() > 4096 {
                        return Err(CoreError::ConnectionError(
                            "Invalid X11 display response".into(),
                        ));
                    }
                    if let Some(environment) = x11_display_environment(&output)? {
                        break environment;
                    }
                }
                Some(
                    russh::ChannelMsg::Failure | russh::ChannelMsg::Close | russh::ChannelMsg::Eof,
                )
                | None => {
                    return Err(CoreError::ConnectionError(
                        "The SSH server did not establish the dedicated X11 display".into(),
                    ));
                }
                _ => {}
            }
        };
        let cancel = self.lifetime.0.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    message = channel.wait() => if matches!(message, None | Some(russh::ChannelMsg::Close | russh::ChannelMsg::Eof)) {
                        cancel.cancel();
                        break;
                    },
                }
            }
        });
        self.forward_tasks.lock().await.push(task);
        Ok(environment)
    }

    async fn prepare_x11_icons(&self) -> CoreResult<Option<String>> {
        let source = include_str!("../../../../../tools/x11-icons.py").replace('\'', "'\\''");
        let install = format!(
            "command -v python3 >/dev/null 2>&1 || exit 0; umask 077; \
             d=$(mktemp -d /tmp/aetherdesk-x11-XXXXXXXX) || exit 0; \
             printf '%s' '{source}' >\"$d/icons.py\" && printf '%s' \"$d/icons.py\""
        );
        let output = self
            .execute_command_output(&install, Duration::from_secs(5))
            .await?;
        let path = output.stdout_text().trim().to_owned();
        if output.exit_status != 0
            || !path.starts_with("/tmp/aetherdesk-x11-")
            || !path.ends_with("/icons.py")
        {
            return Ok(None);
        }
        let path = path.replace('\'', "'\\''");
        // Load and remove the private temporary file before monitoring windows.
        // The helper inherits this shell's DISPLAY and exits when SSH closes it.
        Ok(Some(format!(
            "(AETHERDESK_X11_PARENT=$$ python3 -c 'import os,sys; p=sys.argv[1]; s=open(p).read(); os.unlink(p); os.rmdir(os.path.dirname(p)); exec(compile(s,p,\"exec\"))' '{path}' </dev/null >/dev/null 2>&1 &)"
        )))
    }

    /// Send data to the remote terminal
    pub fn send_data(&self, data: &[u8]) -> CoreResult<()> {
        let channel_guard = self.channel.lock();
        if let Some(channel) = channel_guard.as_ref() {
            channel.write(data)?;
            debug!("Sent {} bytes to channel", data.len());
            Ok(())
        } else {
            warn!("No channel open, cannot send data");
            Err(CoreError::ConnectionError("No channel open".to_string()))
        }
    }

    /// Read all terminal data currently available without blocking the caller.
    pub fn read_available(&self, limit: usize) -> CoreResult<Vec<u8>> {
        let channel_guard = self.channel.lock();
        let Some(channel) = channel_guard.as_ref() else {
            return Err(CoreError::ConnectionError("No channel open".to_string()));
        };

        let data = channel.read(limit)?;
        if data.is_empty() && self.lifetime.0.is_cancelled() {
            return Err(CoreError::ConnectionError(
                "The SSH connection stopped".into(),
            ));
        }
        Ok(data)
    }

    /// Resize the terminal
    pub fn resize_pty(&self, cols: u16, rows: u16) -> CoreResult<()> {
        debug!("Resizing PTY to {}x{}", cols, rows);
        let channel_guard = self.channel.lock();
        if let Some(channel) = channel_guard.as_ref() {
            channel.resize(cols, rows)?;
            Ok(())
        } else {
            Err(CoreError::ConnectionError("No channel open".to_string()))
        }
    }

    /// Close the session
    pub async fn close(&self) -> CoreResult<()> {
        // Cancel both links before waiting on either one.
        self.lifetime.0.cancel();
        if let Some(bulk) = &self.bulk {
            bulk.lifetime.0.cancel();
            let (interactive, graphics) = tokio::join!(self.close_single(), bulk.close_single());
            interactive.and(graphics)
        } else {
            self.close_single().await
        }
    }

    async fn close_single(&self) -> CoreResult<()> {
        debug!("Closing SSH session");
        self.lifetime.0.cancel();
        self.channel.lock().take();
        for task in self.forward_tasks.lock().await.drain(..) {
            task.abort();
        }
        match timeout(
            Duration::from_secs(1),
            self.session
                .disconnect(russh::Disconnect::ByApplication, "", ""),
        )
        .await
        {
            Ok(Ok(_)) => {
                info!("SSH session closed");
                Ok(())
            }
            e => {
                warn!("Error closing session: {:?}", e);
                Ok(()) // Still consider it closed
            }
        }
    }

    /// Bind an ephemeral loopback port and forward every accepted connection
    /// through SSH to the requested host and port. The listener is stopped
    /// automatically when the SSH session is disconnected.
    pub async fn start_local_forward(
        &self,
        local_host: &str,
        remote_host: &str,
        remote_port: u16,
    ) -> CoreResult<u16> {
        let local_address = IpAddr::from_str(local_host).map_err(|_| {
            CoreError::ConnectionError("The SSH tunnel bind address is invalid".to_string())
        })?;
        if !local_address.is_loopback() {
            return Err(CoreError::ConnectionError(
                "SSH tunnels may only bind to a loopback address".to_string(),
            ));
        }
        let listener = TcpListener::bind((local_address, 0))
            .await
            .map_err(|error| {
                CoreError::ConnectionError(format!(
                    "Could not create the local SSH tunnel: {error}"
                ))
            })?;
        let local_port = listener
            .local_addr()
            .map_err(|error| {
                CoreError::ConnectionError(format!(
                    "Could not inspect the local SSH tunnel: {error}"
                ))
            })?
            .port();
        let remote_host = remote_host.to_string();
        let session = self.session.clone();
        let cancelled = self.lifetime.0.clone();
        let task = tokio::spawn(async move {
            let _ = cancelled
                .run_until_cancelled(async {
                    loop {
                        let (mut local_stream, peer) = match listener.accept().await {
                            Ok(connection) => connection,
                            Err(error) => {
                                warn!("SSH tunnel listener stopped: {error}");
                                break;
                            }
                        };
                        if let Err(error) = local_stream.set_nodelay(true) {
                            warn!("SSH tunnel could not disable TCP delay: {error}");
                            continue;
                        }
                        let session = session.clone();
                        let remote_host = remote_host.clone();
                        let cancelled = cancelled.clone();
                        tokio::spawn(async move {
                            let _ = cancelled.run_until_cancelled(async {
                    let channel = match session
                        .channel_open_direct_tcpip(
                            remote_host,
                            u32::from(remote_port),
                            peer.ip().to_string(),
                            u32::from(peer.port()),
                        )
                        .await
                    {
                        Ok(channel) => channel,
                        Err(error) => {
                            warn!("SSH tunnel could not open the remote endpoint: {error}");
                            return;
                        }
                    };
                    let mut ssh_stream = channel.into_stream();
                    if let Err(error) =
                        tokio::io::copy_bidirectional(&mut local_stream, &mut ssh_stream).await
                    {
                        debug!("SSH tunnel stream closed: {error}");
                    }
                    }).await;
                        });
                    }
                })
                .await;
        });
        self.forward_tasks.lock().await.push(task);
        Ok(local_port)
    }

    /// Execute a command and return the output
    pub async fn execute_command(&self, command: &str) -> CoreResult<String> {
        let output = self
            .execute_command_output(command, Duration::from_secs(15))
            .await?;
        let mut combined = output.stdout;
        combined.extend_from_slice(&output.stderr);
        Ok(String::from_utf8_lossy(&combined).into_owned())
    }

    /// Execute a command with separate output streams and a trustworthy exit status.
    pub async fn execute_command_output(
        &self,
        command: &str,
        command_timeout: Duration,
    ) -> CoreResult<CommandOutput> {
        if command.contains('\0') || command_timeout.is_zero() {
            return Err(CoreError::ConnectionError(
                "The remote command or timeout is invalid".to_string(),
            ));
        }
        timeout(command_timeout, self.execute_command_inner(command))
            .await
            .map_err(|_| CoreError::ConnectionError("The remote command timed out".to_string()))?
    }

    /// Execute an approved, finite AI/Tools command in an owned Linux process
    /// group. Cancellation, transport loss, timeout and output overflow close
    /// its stdin lease; the supervisor then cleans up the group's descendants.
    /// Agent launch and retained desktop processes deliberately use the ordinary
    /// command API instead.
    pub async fn execute_scoped_command_output(
        &self,
        command: &str,
        command_timeout: Duration,
    ) -> CoreResult<CommandOutput> {
        if command.contains('\0') || command.len() > 128 * 1024 || command_timeout.is_zero() {
            return Err(CoreError::ConnectionError("Invalid scoped command".into()));
        }
        let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
        let wrapper = format!(
            "exec bash -c {} aetherdesk-job {}",
            quote(include_str!("scoped_command.sh")),
            quote(command)
        );
        let operation = async {
            let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
            channel.exec(true, wrapper.as_str()).await?;
            let mut writer = channel.make_writer();
            writer.write_all(b"AETHER_RUN\n").await?;
            writer.flush().await?;
            let messages = futures::stream::unfold(channel, |mut channel| async move {
                channel.wait().await.map(|message| (message, channel))
            });
            let collect = collect_command_output(messages);
            tokio::pin!(collect);
            let mut pulse = tokio::time::interval(Duration::from_secs(1));
            pulse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = self.lifetime.0.cancelled() => return Err(CoreError::ConnectionError("SSH command cancelled: connection closed".into())),
                    result = &mut collect => return result,
                    _ = pulse.tick() => {
                        timeout(Duration::from_secs(2), writer.write_all(b"AETHER_PULSE\n")).await
                            .map_err(|_| CoreError::ConnectionError("SSH command lease stalled".into()))??;
                        timeout(Duration::from_secs(2), writer.flush()).await
                            .map_err(|_| CoreError::ConnectionError("SSH command lease stalled".into()))??;
                    }
                }
            }
        };
        timeout(command_timeout, operation).await.map_err(|_| {
            CoreError::ConnectionError(
                "The remote command timed out; its process group is being stopped".into(),
            )
        })?
    }

    async fn execute_command_inner(&self, command: &str) -> CoreResult<CommandOutput> {
        debug!("Executing SSH command on a dedicated channel");

        // Open a new channel for command execution
        match self.session.channel_open_session().await {
            Ok(channel) => {
                let channel = OwnedChannel(Some(channel));
                channel.exec(true, command).await?;

                let messages = futures::stream::unfold(channel, |mut channel| async move {
                    channel.wait().await.map(|message| (message, channel))
                });
                collect_command_output(messages).await
            }
            Err(e) => {
                error!("Failed to execute command: {}", e);
                Err(CoreError::ConnectionError(format!(
                    "Command execution failed: {}",
                    e
                )))
            }
        }
    }

    /// Open a raw non-interactive SSH session channel.
    pub async fn open_exec_channel(
        &self,
        command: &str,
    ) -> CoreResult<russh::Channel<ssh_client::Msg>> {
        if command.contains('\0') {
            return Err(CoreError::ConnectionError(
                "The remote command contains a NUL byte".to_string(),
            ));
        }
        let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
        channel.exec(true, command).await?;
        Ok(channel.into_inner())
    }

    /// Open a direct TCP channel through SSH. Remote agent endpoints are
    /// restricted to loopback so they can never be exposed by this API.
    pub async fn open_remote_loopback_channel(
        &self,
        remote_port: u16,
    ) -> CoreResult<russh::Channel<ssh_client::Msg>> {
        if remote_port == 0 {
            return Err(CoreError::ConnectionError(
                "The remote agent port is invalid".to_string(),
            ));
        }
        self.session
            .channel_open_direct_tcpip("127.0.0.1", u32::from(remote_port), "127.0.0.1", 0)
            .await
            .map_err(Into::into)
    }

    /// Video/file traffic uses its own encrypted TCP flow, not just another channel.
    pub async fn open_bulk_loopback_channel(
        &self,
        remote_port: u16,
    ) -> CoreResult<russh::Channel<ssh_client::Msg>> {
        self.bulk
            .as_deref()
            .unwrap_or(self)
            .open_remote_loopback_channel(remote_port)
            .await
    }

    /// List a remote directory through the SSH SFTP subsystem.
    pub async fn sftp_list(&self, path: &str) -> CoreResult<Vec<SftpEntry>> {
        let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
        channel.request_subsystem(true, "sftp").await?;
        let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
        sftp.read_dir(path).await
    }

    /// Upload a local file to the remote host through SFTP.
    pub async fn sftp_upload(&self, local_path: &str, remote_path: &str) -> CoreResult<()> {
        self.sftp_upload_resumable(local_path, remote_path, false)
            .await
    }

    /// Upload a file, optionally continuing from the existing remote size.
    pub async fn sftp_upload_resumable(
        &self,
        local_path: &str,
        remote_path: &str,
        resume: bool,
    ) -> CoreResult<()> {
        let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
        channel.request_subsystem(true, "sftp").await?;
        let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
        sftp.upload(local_path, remote_path, resume).await
    }

    /// Return a remote file's size, or `None` when it does not exist.
    pub async fn sftp_file_size(&self, remote_path: &str) -> CoreResult<Option<u64>> {
        let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
        channel.request_subsystem(true, "sftp").await?;
        let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
        sftp.stat_size(remote_path).await
    }

    /// Download a remote file to the local machine through SFTP.
    pub async fn sftp_download(&self, remote_path: &str, local_path: &str) -> CoreResult<()> {
        self.sftp_download_resumable(remote_path, local_path, false)
            .await
    }

    /// Download a file, optionally continuing from the existing local size.
    pub async fn sftp_download_resumable(
        &self,
        remote_path: &str,
        local_path: &str,
        resume: bool,
    ) -> CoreResult<()> {
        let channel = OwnedChannel(Some(self.session.channel_open_session().await?));
        channel.request_subsystem(true, "sftp").await?;
        let mut sftp = SftpProtocol::connect(channel.into_inner().into_stream()).await?;
        sftp.download(remote_path, local_path, resume).await
    }

    /// Disconnect the session (alias for close)
    pub async fn disconnect(&self) -> CoreResult<()> {
        self.close().await
    }
}

const SSH_FXP_INIT: u8 = 1;
const SSH_FXP_VERSION: u8 = 2;
const SSH_FXP_OPEN: u8 = 3;
const SSH_FXP_CLOSE: u8 = 4;
const SSH_FXP_READ: u8 = 5;
const SSH_FXP_WRITE: u8 = 6;
const SSH_FXP_OPENDIR: u8 = 11;
const SSH_FXP_READDIR: u8 = 12;
const SSH_FXP_STAT: u8 = 17;
const SSH_FXP_STATUS: u8 = 101;
const SSH_FXP_HANDLE: u8 = 102;
const SSH_FXP_DATA: u8 = 103;
const SSH_FXP_NAME: u8 = 104;
const SSH_FXP_ATTRS: u8 = 105;
const SSH_FX_OK: u32 = 0;
const SSH_FX_EOF: u32 = 1;
const SSH_FXF_READ: u32 = 0x0000_0001;
const SSH_FXF_WRITE: u32 = 0x0000_0002;
const SSH_FXF_CREAT: u32 = 0x0000_0008;
const SSH_FXF_TRUNC: u32 = 0x0000_0010;
const SFTP_WRITE_CHUNK_SIZE: usize = 32 * 1024;
const SFTP_READ_CHUNK_SIZE: usize = 256 * 1024;
const SFTP_WRITE_PIPELINE_DEPTH: usize = 32;

/// EOF ends output, but OpenSSH can send the command exit status afterward.
/// Keep receiving channel requests until CLOSE (or the channel disappears).
async fn collect_command_output(
    messages: impl futures::Stream<Item = russh::ChannelMsg>,
) -> CoreResult<CommandOutput> {
    use futures::StreamExt;
    futures::pin_mut!(messages);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    while let Some(msg) = messages.next().await {
        match msg {
            russh::ChannelMsg::Data { ref data } => append_bounded(&mut stdout, data)?,
            russh::ChannelMsg::ExtendedData { ref data, .. } => {
                append_bounded(&mut stderr, data)?;
            }
            russh::ChannelMsg::ExitStatus {
                exit_status: status,
            } => {
                debug!("Command exited with status: {}", status);
                exit_status = Some(status);
            }
            russh::ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(CommandOutput {
        stdout,
        stderr,
        exit_status: exit_status.unwrap_or(255),
    })
}

const MAX_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

fn append_bounded(output: &mut Vec<u8>, data: &[u8]) -> CoreResult<()> {
    let new_length = output
        .len()
        .checked_add(data.len())
        .ok_or_else(|| CoreError::ConnectionError("Remote command output overflowed".into()))?;
    if new_length > MAX_COMMAND_OUTPUT_BYTES {
        return Err(CoreError::ConnectionError(format!(
            "Remote command output exceeded {MAX_COMMAND_OUTPUT_BYTES} bytes"
        )));
    }
    output.extend_from_slice(data);
    Ok(())
}

/// Minimal sequential SFTP v3 client used by the native bridge. Keeping the
/// implementation here avoids external helper executables and preserves the
/// standalone packaging requirement.
struct SftpProtocol<S> {
    stream: S,
    next_request_id: u32,
    extensions: std::collections::HashMap<String, String>,
    progress: Option<Arc<FileProgress>>,
}

impl<S> SftpProtocol<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    async fn connect(stream: S) -> CoreResult<Self> {
        let mut client = Self {
            stream,
            next_request_id: 1,
            extensions: Default::default(),
            progress: None,
        };

        let mut init = Vec::new();
        put_u32(&mut init, 3);
        client.send_packet(SSH_FXP_INIT, &init).await?;
        let (packet_type, payload) = client.read_packet().await?;
        if packet_type != SSH_FXP_VERSION {
            return Err(sftp_error(format!(
                "expected SFTP version packet, received type {packet_type}"
            )));
        }
        let mut cursor = PacketCursor::new(&payload);
        let version = cursor.read_u32()?;
        if version < 3 {
            return Err(sftp_error(format!("unsupported SFTP version {version}")));
        }

        while !cursor.remaining().is_empty() {
            let name = String::from_utf8_lossy(cursor.read_string()?).into_owned();
            let value = String::from_utf8_lossy(cursor.read_string()?).into_owned();
            client.extensions.insert(name, value);
        }
        Ok(client)
    }

    async fn read_dir(&mut self, path: &str) -> CoreResult<Vec<SftpEntry>> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        let (packet_type, response) = self.request(SSH_FXP_OPENDIR, payload).await?;
        if packet_type != SSH_FXP_HANDLE {
            return Err(self.response_error(packet_type, &response, "open directory")?);
        }
        let mut cursor = PacketCursor::new(&response);
        let handle = cursor.read_string()?.to_vec();
        let mut entries = Vec::new();

        loop {
            let mut payload = Vec::new();
            put_string(&mut payload, &handle)?;
            let (packet_type, response) = self.request(SSH_FXP_READDIR, payload).await?;
            if packet_type == SSH_FXP_STATUS {
                let status = parse_status(&response)?;
                if status.code == SSH_FX_EOF {
                    break;
                }
                return Err(sftp_error(format!(
                    "read directory failed: {} (code {})",
                    status.message, status.code
                )));
            }
            if packet_type != SSH_FXP_NAME {
                return Err(sftp_error(format!(
                    "read directory returned unexpected packet type {packet_type}"
                )));
            }

            let mut cursor = PacketCursor::new(&response);
            let count = cursor.read_u32()?;
            if count == 0 {
                return Err(sftp_error(
                    "server returned an empty directory page without EOF",
                ));
            }
            for _ in 0..count {
                let raw_name = cursor.read_string()?;
                if raw_name.is_empty() || raw_name.contains(&b'/') || raw_name.contains(&0) {
                    return Err(sftp_error("invalid directory entry name"));
                }
                let long_name = String::from_utf8_lossy(cursor.read_string()?).into_owned();
                let attrs = cursor.read_attrs()?;
                let name = std::str::from_utf8(raw_name).ok();
                if matches!(name, Some("." | "..")) {
                    continue;
                }
                if entries.len() >= 100_000 {
                    return Err(sftp_error("directory entry limit exceeded"));
                }
                let (name, entry_path, kind) = match name {
                    Some(name) => (
                        name.to_owned(),
                        join_remote_path(path, name),
                        attrs.kind(&long_name),
                    ),
                    None => {
                        warn!("Directory contains a non-UTF-8 filename; listing it as unavailable without changing its bytes");
                        (
                            format!(
                                "[Non-UTF-8 filename: {}]",
                                raw_name
                                    .iter()
                                    .take(48)
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<String>()
                            ),
                            String::new(),
                            "unsupported-name".to_owned(),
                        )
                    }
                };
                entries.push(SftpEntry {
                    path: entry_path,
                    kind,
                    name,
                    size: attrs.size,
                    permissions: attrs.permissions,
                    modified: attrs.modified,
                });
            }
        }

        self.close_handle(&handle).await?;
        entries.sort_by(|left, right| {
            let left_dir = left.kind == "directory";
            let right_dir = right.kind == "directory";
            right_dir
                .cmp(&left_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        Ok(entries)
    }

    async fn upload(
        &mut self,
        local_path: &str,
        remote_path: &str,
        resume: bool,
    ) -> CoreResult<()> {
        let mut local_file = tokio::fs::File::open(local_path).await?;
        let local_size = local_file.metadata().await?.len();
        let mut offset = if resume {
            self.stat_size(remote_path).await?.unwrap_or(0)
        } else {
            0
        };
        if offset > local_size {
            offset = 0;
        }
        if offset == local_size && local_size > 0 {
            return Ok(());
        }
        if offset > 0 {
            local_file.seek(SeekFrom::Start(offset)).await?;
        }
        let flags = SSH_FXF_WRITE | SSH_FXF_CREAT | if offset == 0 { SSH_FXF_TRUNC } else { 0 };
        let handle = self.open_file(remote_path, flags).await?;
        self.update_progress(offset, local_size)?;
        let mut buffer = vec![0_u8; SFTP_WRITE_CHUNK_SIZE];
        let mut pending = HashSet::new();

        loop {
            let count = local_file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let mut payload = Vec::new();
            put_string(&mut payload, &handle)?;
            put_u64(&mut payload, offset);
            put_string(&mut payload, &buffer[..count])?;
            pending.insert(self.send_request(SSH_FXP_WRITE, payload).await?);
            offset += count as u64;
            self.update_progress(offset, local_size)?;

            if pending.len() >= SFTP_WRITE_PIPELINE_DEPTH {
                self.receive_write_status(&mut pending).await?;
            }
        }

        while !pending.is_empty() {
            self.receive_write_status(&mut pending).await?;
        }

        self.close_handle(&handle).await
    }

    async fn download(
        &mut self,
        remote_path: &str,
        local_path: &str,
        resume: bool,
    ) -> CoreResult<()> {
        let remote_size = self.stat_size(remote_path).await?;
        let handle = self.open_file(remote_path, SSH_FXF_READ).await?;
        let existing_size = if resume {
            tokio::fs::metadata(local_path)
                .await
                .map(|metadata| metadata.len())
                .unwrap_or(0)
        } else {
            0
        };
        let mut offset = match remote_size {
            Some(total) if existing_size < total => existing_size,
            Some(total) if existing_size == total && total > 0 => {
                self.close_handle(&handle).await?;
                return Ok(());
            }
            _ => 0,
        };
        let mut local_file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(offset == 0)
            .open(local_path)
            .await?;
        if offset > 0 {
            local_file.seek(SeekFrom::Start(offset)).await?;
        }

        loop {
            let mut payload = Vec::new();
            put_string(&mut payload, &handle)?;
            put_u64(&mut payload, offset);
            put_u32(&mut payload, SFTP_READ_CHUNK_SIZE as u32);
            let (packet_type, response) = self.request(SSH_FXP_READ, payload).await?;

            if packet_type == SSH_FXP_STATUS {
                let status = parse_status(&response)?;
                if status.code == SSH_FX_EOF {
                    break;
                }
                return Err(sftp_error(format!(
                    "read remote file failed: {} (code {})",
                    status.message, status.code
                )));
            }
            if packet_type != SSH_FXP_DATA {
                return Err(sftp_error(format!(
                    "read remote file returned unexpected packet type {packet_type}"
                )));
            }

            let mut cursor = PacketCursor::new(&response);
            let data = cursor.read_string()?;
            if data.is_empty() {
                break;
            }
            local_file.write_all(data).await?;
            offset += data.len() as u64;
            self.update_progress(offset, remote_size.unwrap_or(offset))?;
        }

        local_file.flush().await?;
        self.close_handle(&handle).await
    }

    async fn stat_size(&mut self, path: &str) -> CoreResult<Option<u64>> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        let (packet_type, response) = self.request(SSH_FXP_STAT, payload).await?;
        if packet_type == SSH_FXP_STATUS {
            let status = parse_status(&response)?;
            if status.code == 2 {
                return Ok(None);
            }
            return Err(sftp_error(format!(
                "stat failed: {} (code {})",
                status.message, status.code
            )));
        }
        if packet_type != SSH_FXP_ATTRS {
            return Err(sftp_error(format!(
                "stat returned unexpected packet type {packet_type}"
            )));
        }
        Ok(PacketCursor::new(&response).read_attrs()?.size)
    }

    async fn open_file(&mut self, path: &str, flags: u32) -> CoreResult<Vec<u8>> {
        let mut payload = Vec::new();
        put_string(&mut payload, path.as_bytes())?;
        put_u32(&mut payload, flags);
        put_u32(&mut payload, 0); // Empty ATTRS structure.
        let (packet_type, response) = self.request(SSH_FXP_OPEN, payload).await?;
        if packet_type != SSH_FXP_HANDLE {
            return Err(self.response_error(packet_type, &response, "open remote file")?);
        }
        let mut cursor = PacketCursor::new(&response);
        Ok(cursor.read_string()?.to_vec())
    }

    async fn close_handle(&mut self, handle: &[u8]) -> CoreResult<()> {
        let mut payload = Vec::new();
        put_string(&mut payload, handle)?;
        let (packet_type, response) = self.request(SSH_FXP_CLOSE, payload).await?;
        self.expect_ok(packet_type, &response, "close SFTP handle")
    }

    async fn request(&mut self, packet_type: u8, payload: Vec<u8>) -> CoreResult<(u8, Vec<u8>)> {
        let request_id = self.send_request(packet_type, payload).await?;
        let (response_id, response_type, response) = self.receive_response().await?;
        if response_id != request_id {
            return Err(sftp_error(format!(
                "SFTP response id mismatch: expected {request_id}, received {response_id}"
            )));
        }
        Ok((response_type, response))
    }

    async fn send_request(&mut self, packet_type: u8, mut payload: Vec<u8>) -> CoreResult<u32> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let mut request = Vec::with_capacity(payload.len() + 4);
        put_u32(&mut request, request_id);
        request.append(&mut payload);
        self.send_packet(packet_type, &request).await?;
        Ok(request_id)
    }

    async fn receive_response(&mut self) -> CoreResult<(u32, u8, Vec<u8>)> {
        let (response_type, response) = self.read_packet().await?;
        let mut cursor = PacketCursor::new(&response);
        let response_id = cursor.read_u32()?;
        Ok((response_id, response_type, cursor.remaining().to_vec()))
    }

    async fn receive_write_status(&mut self, pending: &mut HashSet<u32>) -> CoreResult<()> {
        let (response_id, packet_type, response) = self.receive_response().await?;
        if !pending.remove(&response_id) {
            return Err(sftp_error(format!(
                "received an unexpected SFTP response id {response_id} during upload"
            )));
        }
        self.expect_ok(packet_type, &response, "write remote file")
    }

    async fn send_packet(&mut self, packet_type: u8, payload: &[u8]) -> CoreResult<()> {
        let length = payload
            .len()
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| sftp_error("SFTP packet is too large"))?;
        self.stream.write_all(&length.to_be_bytes()).await?;
        self.stream.write_all(&[packet_type]).await?;
        self.stream.write_all(payload).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn read_packet(&mut self) -> CoreResult<(u8, Vec<u8>)> {
        let length = self.stream.read_u32().await? as usize;
        if length == 0 || length > 16 * 1024 * 1024 {
            return Err(sftp_error(format!("invalid SFTP packet length {length}")));
        }
        let packet_type = self.stream.read_u8().await?;
        let mut payload = vec![0_u8; length - 1];
        self.stream.read_exact(&mut payload).await?;
        Ok((packet_type, payload))
    }

    fn expect_ok(&self, packet_type: u8, payload: &[u8], operation: &str) -> CoreResult<()> {
        if packet_type != SSH_FXP_STATUS {
            return Err(sftp_error(format!(
                "{operation} returned unexpected packet type {packet_type}"
            )));
        }
        let status = parse_status(payload)?;
        if status.code == SSH_FX_OK {
            Ok(())
        } else {
            Err(sftp_error(format!(
                "{operation} failed: {} (code {})",
                status.message, status.code
            )))
        }
    }

    fn response_error(
        &self,
        packet_type: u8,
        payload: &[u8],
        operation: &str,
    ) -> CoreResult<CoreError> {
        if packet_type == SSH_FXP_STATUS {
            let status = parse_status(payload)?;
            Ok(sftp_error(format!(
                "{operation} failed: {} (code {})",
                status.message, status.code
            )))
        } else {
            Ok(sftp_error(format!(
                "{operation} returned unexpected packet type {packet_type}"
            )))
        }
    }
}

#[derive(Debug)]
struct SftpStatus {
    code: u32,
    message: String,
}

#[derive(Debug, Default)]
struct SftpAttrs {
    size: Option<u64>,
    permissions: Option<u32>,
    modified: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
}

impl SftpAttrs {
    fn kind(&self, long_name: &str) -> String {
        if let Some(mode) = self.permissions {
            return match mode & 0o170000 {
                0o040000 => "directory",
                0o120000 => "symlink",
                0o100000 => "file",
                _ => "other",
            }
            .to_string();
        }
        match long_name.as_bytes().first().copied() {
            Some(b'd') => "directory",
            Some(b'l') => "symlink",
            _ => "file",
        }
        .to_string()
    }
}

struct PacketCursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> PacketCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        &self.data[self.position..]
    }

    fn take(&mut self, length: usize) -> CoreResult<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| sftp_error("SFTP packet offset overflow"))?;
        if end > self.data.len() {
            return Err(sftp_error("truncated SFTP packet"));
        }
        let value = &self.data[self.position..end];
        self.position = end;
        Ok(value)
    }

    fn read_u32(&mut self) -> CoreResult<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("four-byte slice");
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_u64(&mut self) -> CoreResult<u64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("eight-byte slice");
        Ok(u64::from_be_bytes(bytes))
    }

    fn read_string(&mut self) -> CoreResult<&'a [u8]> {
        let length = self.read_u32()? as usize;
        self.take(length)
    }

    fn read_attrs(&mut self) -> CoreResult<SftpAttrs> {
        let flags = self.read_u32()?;
        let size = if flags & 0x0000_0001 != 0 {
            Some(self.read_u64()?)
        } else {
            None
        };
        let (uid, gid) = if flags & 0x0000_0002 != 0 {
            (Some(self.read_u32()?), Some(self.read_u32()?))
        } else {
            (None, None)
        };
        let permissions = if flags & 0x0000_0004 != 0 {
            Some(self.read_u32()?)
        } else {
            None
        };
        let modified = if flags & 0x0000_0008 != 0 {
            self.read_u32()?;
            Some(self.read_u32()?)
        } else {
            None
        };
        if flags & 0x8000_0000 != 0 {
            let count = self.read_u32()?;
            for _ in 0..count {
                self.read_string()?;
                self.read_string()?;
            }
        }
        Ok(SftpAttrs {
            size,
            permissions,
            modified,
            uid,
            gid,
        })
    }
}

fn parse_status(payload: &[u8]) -> CoreResult<SftpStatus> {
    let mut cursor = PacketCursor::new(payload);
    let code = cursor.read_u32()?;
    let message = if cursor.remaining().is_empty() {
        String::new()
    } else {
        String::from_utf8_lossy(cursor.read_string()?).into_owned()
    };
    Ok(SftpStatus { code, message })
}

fn put_u32(target: &mut Vec<u8>, value: u32) {
    target.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(target: &mut Vec<u8>, value: u64) {
    target.extend_from_slice(&value.to_be_bytes());
}

fn put_string(target: &mut Vec<u8>, value: &[u8]) -> CoreResult<()> {
    let length = u32::try_from(value.len()).map_err(|_| sftp_error("SFTP string is too large"))?;
    put_u32(target, length);
    target.extend_from_slice(value);
    Ok(())
}

fn join_remote_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn sftp_error(message: impl Into<String>) -> CoreError {
    CoreError::ConnectionError(format!("SFTP: {}", message.into()))
}

#[cfg(test)]
mod sftp_tests {
    use super::*;

    #[tokio::test]
    async fn non_utf8_names_do_not_break_listing_or_become_lossy_paths() {
        async fn packet(s: &mut tokio::io::DuplexStream) -> (u8, Vec<u8>) {
            let n = s.read_u32().await.unwrap();
            assert!((1..=4096).contains(&n));
            let kind = s.read_u8().await.unwrap();
            let mut data = vec![0; n as usize - 1];
            s.read_exact(&mut data).await.unwrap();
            (kind, data)
        }
        async fn reply(s: &mut tokio::io::DuplexStream, kind: u8, data: Vec<u8>) {
            s.write_u32(data.len() as u32 + 1).await.unwrap();
            s.write_u8(kind).await.unwrap();
            s.write_all(&data).await.unwrap();
        }
        let (client, mut peer) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            assert_eq!(packet(&mut peer).await.0, SSH_FXP_INIT);
            reply(&mut peer, SSH_FXP_VERSION, 3u32.to_be_bytes().to_vec()).await;
            for step in 0..4 {
                let (kind, data) = packet(&mut peer).await;
                let id = &data[..4];
                let mut result = id.to_vec();
                match step {
                    0 => {
                        assert_eq!(kind, SSH_FXP_OPENDIR);
                        put_string(&mut result, b"handle").unwrap();
                        reply(&mut peer, SSH_FXP_HANDLE, result).await;
                    }
                    1 => {
                        assert_eq!(kind, SSH_FXP_READDIR);
                        put_u32(&mut result, 3);
                        for name in [
                            b"before.txt".as_slice(),
                            b"old-\xff.txt",
                            "after-ñ.txt".as_bytes(),
                        ] {
                            put_string(&mut result, name).unwrap();
                            put_string(&mut result, b"").unwrap();
                            put_u32(&mut result, 0);
                        }
                        reply(&mut peer, SSH_FXP_NAME, result).await;
                    }
                    _ => {
                        assert_eq!(
                            kind,
                            if step == 2 {
                                SSH_FXP_READDIR
                            } else {
                                SSH_FXP_CLOSE
                            }
                        );
                        put_u32(&mut result, if step == 2 { SSH_FX_EOF } else { SSH_FX_OK });
                        put_string(&mut result, b"").unwrap();
                        put_string(&mut result, b"").unwrap();
                        reply(&mut peer, SSH_FXP_STATUS, result).await;
                    }
                }
            }
        });
        let mut protocol = SftpProtocol::connect(client).await.unwrap();
        let entries = protocol.read_dir("/home/test").await.unwrap();
        assert_eq!(entries.len(), 3);
        let invalid = entries
            .iter()
            .find(|e| e.kind == "unsupported-name")
            .unwrap();
        assert!(invalid.path.is_empty());
        assert!(!invalid.name.contains('\u{fffd}'));
        assert!(entries.iter().any(|e| e.path == "/home/test/before.txt"));
        assert!(entries.iter().any(|e| e.path == "/home/test/after-ñ.txt"));
        task.await.unwrap();
    }

    #[test]
    fn parses_sftp_v3_attributes() {
        let mut packet = Vec::new();
        put_u32(&mut packet, 0x0000_0001 | 0x0000_0004 | 0x0000_0008);
        put_u64(&mut packet, 42);
        put_u32(&mut packet, 0o100644);
        put_u32(&mut packet, 100);
        put_u32(&mut packet, 200);

        let attrs = PacketCursor::new(&packet).read_attrs().unwrap();
        assert_eq!(attrs.size, Some(42));
        assert_eq!(attrs.permissions, Some(0o100644));
        assert_eq!(attrs.modified, Some(200));
        assert_eq!(attrs.kind(""), "file");
    }

    #[test]
    fn parses_status_and_remote_paths() {
        let mut packet = Vec::new();
        put_u32(&mut packet, SSH_FX_EOF);
        put_string(&mut packet, b"end of file").unwrap();
        put_string(&mut packet, b"").unwrap();

        let status = parse_status(&packet).unwrap();
        assert_eq!(status.code, SSH_FX_EOF);
        assert_eq!(status.message, "end of file");
        assert_eq!(join_remote_path("/", "logs"), "/logs");
        assert_eq!(join_remote_path("/srv", "logs"), "/srv/logs");
    }

    #[test]
    fn rejects_truncated_packets() {
        let error = PacketCursor::new(&[0, 0, 0]).read_u32().unwrap_err();
        assert!(error.to_string().contains("truncated SFTP packet"));
    }
}

// Handler for SSH client events
pub struct ClientHandler {
    verified_key: Arc<parking_lot::Mutex<Option<String>>>,
    event_bus: Option<Arc<EventBus>>,
    host: String,
    port: u16,
    trust_new_host_key: bool,
    confirmed_host_fingerprint: Option<String>,
    x11: Option<X11ForwardingConfig>,
    cancel: tokio_util::sync::CancellationToken,
    #[cfg(test)]
    pinned_test_key: Option<russh::keys::PublicKey>,
}

fn decode_x11_cookie(cookie: &str) -> std::io::Result<Vec<u8>> {
    if cookie.is_empty() || !cookie.len().is_multiple_of(2) {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "the X11 authorization cookie has an invalid length",
        ));
    }
    (0..cookie.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&cookie[index..index + 2], 16).map_err(|_| {
                IoError::new(
                    ErrorKind::InvalidData,
                    "the X11 authorization cookie is not hexadecimal",
                )
            })
        })
        .collect()
}

async fn proxy_authenticated_x11<S>(
    mut ssh_x11_stream: S,
    mut local_x_server: TcpStream,
    config: X11ForwardingConfig,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // The remote X client begins with a 12-byte X11 setup header. Validate
    // the SSH-only cookie, then substitute VcXsrv's real cookie locally. This
    // prevents the reusable local cookie from ever being stored remotely.
    let mut header = [0_u8; 12];
    ssh_x11_stream.read_exact(&mut header).await?;
    let (protocol_length, cookie_length) = x11_authorization_lengths(&header)?;
    if protocol_length > 256 || cookie_length > 256 {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "the forwarded X11 authorization record is too large",
        ));
    }
    let padded_protocol_length = (protocol_length + 3) & !3;
    let padded_cookie_length = (cookie_length + 3) & !3;
    let mut authorization = vec![0_u8; padded_protocol_length + padded_cookie_length];
    ssh_x11_stream.read_exact(&mut authorization).await?;

    rewrite_x11_authorization(&mut authorization, protocol_length, cookie_length, &config)?;
    local_x_server.write_all(&header).await?;
    local_x_server.write_all(&authorization).await?;
    local_x_server.flush().await?;
    tokio::io::copy_bidirectional(&mut ssh_x11_stream, &mut local_x_server).await?;
    Ok(())
}

fn x11_authorization_lengths(header: &[u8; 12]) -> std::io::Result<(usize, usize)> {
    let little_endian = match header[0] {
        b'l' => true,
        b'B' => false,
        _ => {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                "the forwarded X11 client sent an invalid byte order",
            ))
        }
    };
    let read_u16 = |bytes: [u8; 2]| {
        if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    Ok((
        usize::from(read_u16([header[6], header[7]])),
        usize::from(read_u16([header[8], header[9]])),
    ))
}

fn rewrite_x11_authorization(
    authorization: &mut [u8],
    protocol_length: usize,
    cookie_length: usize,
    config: &X11ForwardingConfig,
) -> std::io::Result<()> {
    let padded_protocol_length = (protocol_length + 3) & !3;
    let padded_cookie_length = (cookie_length + 3) & !3;
    if authorization.len() != padded_protocol_length + padded_cookie_length {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "the forwarded X11 authorization record has an invalid length",
        ));
    }

    let protocol = &authorization[..protocol_length];
    if protocol != config.authentication_protocol.as_bytes() {
        return Err(IoError::new(
            ErrorKind::PermissionDenied,
            "the forwarded X11 authentication protocol was rejected",
        ));
    }
    let remote_cookie = decode_x11_cookie(&config.authentication_cookie)?;
    let local_cookie = decode_x11_cookie(&config.local_authentication_cookie)?;
    let cookie_start = padded_protocol_length;
    let cookie_end = cookie_start + cookie_length;
    if authorization[cookie_start..cookie_end] != remote_cookie
        || local_cookie.len() != cookie_length
    {
        return Err(IoError::new(
            ErrorKind::PermissionDenied,
            "the forwarded X11 authorization cookie was rejected",
        ));
    }
    authorization[cookie_start..cookie_end].copy_from_slice(&local_cookie);
    Ok(())
}

fn verify_confirmed_fingerprint(expected: Option<&str>, actual: &str) -> CoreResult<()> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(CoreError::ConnectionError(
            "The SSH host key changed after confirmation. The connection was refused.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod confirmation_tests {
    use super::*;
    #[test]
    fn a_trust_retry_is_bound_to_the_displayed_fingerprint() {
        assert!(verify_confirmed_fingerprint(Some("SHA256:one"), "SHA256:one").is_ok());
        assert!(verify_confirmed_fingerprint(Some("SHA256:one"), "SHA256:two").is_err());
        // No confirmation still flows through normal known_hosts verification.
        assert!(verify_confirmed_fingerprint(None, "SHA256:two").is_ok());
    }
}

impl ssh_client::Handler for ClientHandler {
    type Error = CoreError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public_key = server_public_key.public_key();
        verify_confirmed_fingerprint(
            self.confirmed_host_fingerprint.as_deref(),
            &public_key.fingerprint(HashAlg::Sha256).to_string(),
        )?;
        #[cfg(test)]
        if let Some(key) = &self.pinned_test_key {
            let accepted = key.key_data() == public_key.key_data();
            if accepted {
                *self.verified_key.lock() =
                    Some(public_key.fingerprint(HashAlg::Sha256).to_string());
            }
            return Ok(accepted);
        }
        match russh::keys::known_hosts::known_host_keys(&self.host, self.port) {
            Ok(keys) if keys.iter().any(|(_, known_key)| known_key == &public_key) => {
                *self.verified_key.lock() = Some(public_key.fingerprint(HashAlg::Sha256).to_string());
                info!("Verified SSH host key for {}:{}", self.host, self.port);
                Ok(true)
            }
            Ok(keys) if keys.is_empty() => {
                let fingerprint = public_key.fingerprint(HashAlg::Sha256);
                if !self.trust_new_host_key {
                    return Err(CoreError::ConnectionError(format!(
                        "AETHERDESK_HOST_KEY_UNKNOWN|{}|{}|{}|{}",
                        self.host,
                        self.port,
                        public_key.algorithm(),
                        fingerprint
                    )));
                }
                russh::keys::known_hosts::learn_known_hosts(&self.host, self.port, &public_key)
                    .map_err(|error| {
                        CoreError::ConnectionError(format!(
                            "could not remember the SSH host key for {}:{}: {error}",
                            self.host, self.port
                        ))
                    })?;
                warn!(
                    "User confirmed SSH host key {} for {}:{}; saved it to known_hosts",
                    fingerprint, self.host, self.port
                );
                *self.verified_key.lock() = Some(fingerprint.to_string());
                Ok(true)
            }
            Ok(_) => Err(CoreError::ConnectionError(format!(
                "SSH host key changed for {}:{}. The connection was refused; verify the server before updating known_hosts.",
                self.host, self.port
            ))),
            Err(error) => Err(CoreError::ConnectionError(format!(
                "SSH host key verification failed for {}:{}: {error}",
                self.host, self.port
            ))),
        }
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        _session: &mut ssh_client::Session,
    ) -> Result<(), Self::Error> {
        debug!("Received {} bytes on channel {:?}", data.len(), channel);
        if let Some(event_bus) = self.event_bus.as_ref() {
            event_bus.emit(ad_events::Event::TerminalData {
                session_id: ad_core::SessionId::new(),
                data: data.to_vec(),
            });
        }
        Ok(())
    }

    fn server_channel_open_x11(
        &mut self,
        channel: russh::Channel<ssh_client::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ssh_client::ChannelOpenHandle,
        _session: &mut ssh_client::Session,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let x11 = self.x11.clone();
        let cancel = self.cancel.clone();
        async move {
            if cancel.is_cancelled() {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            }
            let Some(x11) = x11 else {
                reply
                    .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                    .await;
                return Ok(());
            };
            // Handler callbacks run on the shared SSH transport task. A slow
            // local X server must not suspend terminal traffic on that task.
            tokio::spawn(async move {
                match timeout(
                    Duration::from_secs(5),
                    TcpStream::connect(("127.0.0.1", x11.local_port)),
                )
                .await
                {
                    Ok(Ok(local_x_server)) => {
                        if let Err(error) = local_x_server.set_nodelay(true) {
                            warn!("Could not disable X11 TCP delay: {error}");
                        }
                        reply.accept().await;
                        info!(
                            "Accepted forwarded X11 channel for local display port {}",
                            x11.local_port
                        );
                        tokio::spawn(async move {
                            if let Err(error) = cancel
                                .run_until_cancelled(proxy_authenticated_x11(
                                    channel.into_stream(),
                                    local_x_server,
                                    x11,
                                ))
                                .await
                                .unwrap_or(Ok(()))
                            {
                                warn!("X11 forwarding stream closed: {error}");
                            }
                        });
                    }
                    _ => {
                        reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                    }
                }
            });
            Ok(())
        }
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut ssh_client::Session,
    ) -> Result<(), Self::Error> {
        info!("Auth banner: {}", banner);
        Ok(())
    }
}

#[cfg(test)]
mod x11_tests {
    use super::*;

    #[test]
    fn x11_compresses_only_after_authentication_with_an_uncompressed_fallback() {
        let x11 = client_preferences(true);
        assert_eq!(
            x11.compression.as_ref(),
            &[russh::compression::ZLIB_LEGACY, russh::compression::NONE]
        );
        assert_eq!(
            client_preferences(false).compression[0],
            russh::compression::NONE
        );
        assert!(!x11
            .key
            .iter()
            .any(|key| matches!(key, Algorithm::Rsa { hash: None })));
    }

    fn config() -> X11ForwardingConfig {
        X11ForwardingConfig {
            local_port: 6010,
            authentication_protocol: "MIT-MAGIC-COOKIE-1".to_string(),
            authentication_cookie: "00112233445566778899aabbccddeeff".to_string(),
            local_authentication_cookie: "ffeeddccbbaa99887766554433221100".to_string(),
            screen_number: 0,
        }
    }

    fn authorization(cookie: &[u8]) -> Vec<u8> {
        let protocol = b"MIT-MAGIC-COOKIE-1";
        let mut value = protocol.to_vec();
        value.resize((protocol.len() + 3) & !3, 0);
        value.extend_from_slice(cookie);
        value.resize((value.len() + 3) & !3, 0);
        value
    }

    #[test]
    fn rewrites_spoofed_x11_cookie_for_the_local_server() {
        let config = config();
        let remote_cookie = decode_x11_cookie(&config.authentication_cookie).unwrap();
        let local_cookie = decode_x11_cookie(&config.local_authentication_cookie).unwrap();
        let mut record = authorization(&remote_cookie);

        rewrite_x11_authorization(&mut record, 18, 16, &config).unwrap();

        assert_eq!(&record[20..36], local_cookie.as_slice());
    }

    #[test]
    fn rejects_an_untrusted_x11_cookie() {
        let mut record = authorization(&[0x5a; 16]);
        let error = rewrite_x11_authorization(&mut record, 18, 16, &config()).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn reads_x11_lengths_in_both_byte_orders() {
        let mut little = [0_u8; 12];
        little[0] = b'l';
        little[6..10].copy_from_slice(&[18, 0, 16, 0]);
        assert_eq!(x11_authorization_lengths(&little).unwrap(), (18, 16));

        let mut big = [0_u8; 12];
        big[0] = b'B';
        big[6..10].copy_from_slice(&[0, 18, 0, 16]);
        assert_eq!(x11_authorization_lengths(&big).unwrap(), (18, 16));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use russh::ChannelMsg;

    #[tokio::test]
    async fn reads_exit_status_after_eof() {
        // OpenSSH may end stdout/stderr before sending the command result.
        // Cover both a successful silent mkdir and a real shell failure.
        for status in [0, 1, 127] {
            let output = collect_command_output(futures::stream::iter([
                ChannelMsg::Eof,
                ChannelMsg::ExitStatus {
                    exit_status: status,
                },
                ChannelMsg::Close,
            ]))
            .await
            .unwrap();
            assert_eq!(output.exit_status, status);
            assert!(output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }

    #[tokio::test]
    async fn preserves_output_and_status_before_eof() {
        let output = collect_command_output(futures::stream::iter([
            ChannelMsg::Data {
                data: b"ready".as_slice().into(),
            },
            ChannelMsg::ExtendedData {
                data: b"diagnostic".as_slice().into(),
                ext: 1,
            },
            ChannelMsg::ExitStatus { exit_status: 23 },
            ChannelMsg::Eof,
            ChannelMsg::Close,
        ]))
        .await
        .unwrap();
        assert_eq!(output.stdout, b"ready");
        assert_eq!(output.stderr, b"diagnostic");
        assert_eq!(output.exit_status, 23);
    }

    #[tokio::test]
    async fn missing_status_is_never_reported_as_success() {
        for messages in [
            vec![ChannelMsg::Eof, ChannelMsg::Close],
            vec![ChannelMsg::Eof],
        ] {
            let output = collect_command_output(futures::stream::iter(messages))
                .await
                .unwrap();
            assert_eq!(output.exit_status, 255);
        }
    }

    #[tokio::test]
    async fn eof_without_close_still_obeys_command_timeout() {
        use futures::StreamExt;
        let messages = futures::stream::iter([ChannelMsg::Eof]).chain(futures::stream::pending());
        assert!(
            timeout(Duration::from_millis(10), collect_command_output(messages))
                .await
                .is_err()
        );
    }
}

#[cfg(all(test, unix))]
mod shell_history_tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    #[test]
    fn large_bootstrap_payload_is_not_duplicated_past_the_pty_line_limit() {
        let command = format!("__ad_value='{}'; :", "x".repeat(1600));
        let input = private_shell_bootstrap(&command);
        assert_eq!(input.matches(command.as_str()).count(), 1);
        assert!(
            input.len() < 4096,
            "startup would be truncated before Readline takes over"
        );
    }

    #[test]
    fn bootstrap_is_parseable_by_a_posix_shell() {
        let mut child = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(
            input,
            "{}",
            private_shell_bootstrap(":").trim_end_matches('\r')
        )
        .unwrap();
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{:?}", output);
    }

    #[test]
    fn bootstrap_never_appears_in_bash_history_or_removes_user_commands() {
        let command = "__ad_test_ready='ready with quotes'; :";
        for control in ["", "ignoredups", "ignorespace", "ignoreboth:erasedups"] {
            for ignore in ["", "*aetherdesk*"] {
                for history_enabled in [true, false] {
                    for append in [true, false] {
                        let temporary = tempfile::tempdir().unwrap();
                        let history = temporary.path().join("history");
                        let dump = temporary.path().join("active-history");
                        let settings = temporary.path().join("settings");
                        // Include two old bootstrap entries separated by real work,
                        // and a real command mentioning the integration function.
                        std::fs::write(
                            &history,
                            format!(
                                "echo keep-before\n {command}; stty echo\n\
                             echo __aetherdesk_emit_cwd\n {command}; stty echo\n\
                             echo keep-after\n"
                            ),
                        )
                        .unwrap();
                        let mut child = Command::new(
                            std::env::var("AD_TEST_BASH").unwrap_or_else(|_| "bash".into()),
                        )
                        .args([
                            "--noprofile",
                            "--norc",
                            if append { "-O" } else { "+O" },
                            "histappend",
                            "-i",
                        ])
                        .env("HISTFILE", &history)
                        .env("HISTSIZE", "1000")
                        .env("HISTFILESIZE", "1000")
                        .env("HISTCONTROL", control)
                        .env("HISTIGNORE", ignore)
                        .env("HISTTIMEFORMAT", "%F %T ")
                        .env("AD_TEST_DUMP", &dump)
                        .env("AD_TEST_SETTINGS", &settings)
                        .env("PS1", "")
                        .env("PROMPT_COMMAND", "")
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap();
                        let mut input = child.stdin.take().unwrap();
                        if !history_enabled {
                            writeln!(input, "set +o history").unwrap();
                        }
                        let bootstrap = private_shell_bootstrap(command);
                        writeln!(input, "{}", bootstrap.trim_end_matches('\r')).unwrap();
                        writeln!(input, "printf '%s' \"$__ad_test_ready\"").unwrap();
                        writeln!(input, "HISTTIMEFORMAT= builtin history > \"$AD_TEST_DUMP\"")
                            .unwrap();
                        writeln!(input, "printf '%s\\n' \"$HISTCONTROL\" \"$HISTIGNORE\" \"$HISTTIMEFORMAT\" > \"$AD_TEST_SETTINGS\"").unwrap();
                        writeln!(input, "shopt -p histappend >> \"$AD_TEST_SETTINGS\" || :")
                            .unwrap();
                        writeln!(input, "set -o | grep '^history' >> \"$AD_TEST_SETTINGS\"")
                            .unwrap();
                        writeln!(input, "exit").unwrap();
                        drop(input);
                        let output = child.wait_with_output().unwrap();
                        assert!(output.status.success(), "{:?}", output);
                        assert_eq!(output.stdout, b"ready with quotes");
                        let active = std::fs::read_to_string(dump).unwrap();
                        for keep in [
                            "echo keep-before",
                            "echo keep-after",
                            "echo __aetherdesk_emit_cwd",
                        ] {
                            assert!(active.contains(keep), "Missing user command: {active}");
                        }
                        let saved = std::fs::read_to_string(&history).unwrap();
                        assert!(!active.contains(command), "{active}");
                        for text in [&active, &saved] {
                            assert!(!text.contains("aetherdesk-bootstrap-"), "{text}");
                            assert!(!text.contains("__aetherdesk_clean_history"), "{text}");
                        }
                        if history_enabled {
                            assert!(
                                saved.contains("printf '%s' \"$__ad_test_ready\""),
                                "First user command missing: {saved}"
                            );
                        }
                        let actual_settings = std::fs::read_to_string(settings).unwrap();
                        assert!(
                            actual_settings.starts_with(&format!("{control}\n{ignore}\n%F %T \n"))
                        );
                        assert!(actual_settings.contains(if append {
                            "shopt -s histappend"
                        } else {
                            "shopt -u histappend"
                        }));
                        assert!(actual_settings
                            .lines()
                            .last()
                            .unwrap()
                            .ends_with(if history_enabled { "on" } else { "off" }));
                    }
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
fn sftp_test_server_path() -> String {
    std::env::var("AD_SFTP_SERVER").unwrap_or_else(|_| {
        [
            "/usr/lib/openssh/sftp-server",
            "/usr/libexec/openssh/sftp-server",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .unwrap_or("/usr/lib/openssh/sftp-server")
        .into()
    })
}
#[cfg(all(test, unix))]
mod sftp_integration {
    use super::*;
    #[tokio::test]
    async fn openssh_sftp_upload_download_resume_and_empty_files() {
        let server = sftp_test_server_path();
        assert!(
            std::path::Path::new(&server).exists(),
            "Install openssh-sftp-server to run the SFTP integration test"
        );
        let mut child = tokio::process::Command::new(server)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (client, bridge) = tokio::io::duplex(1024 * 1024);
        let (mut read, mut write) = tokio::io::split(bridge);
        let proxy = tokio::spawn(async move {
            let _ = tokio::join!(
                tokio::io::copy(&mut read, &mut input),
                tokio::io::copy(&mut output, &mut write)
            );
        });
        let mut sftp = SftpProtocol::connect(client).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().join("local input.bin");
        let remote = directory.path().join("remote spaced ñ.bin");
        let download = directory.path().join("download.bin");
        let data: Vec<u8> = (0..2 * 1024 * 1024 + 317).map(|i| (i * 23) as u8).collect();
        tokio::fs::write(&local, &data).await.unwrap();
        let (local, remote, download) = (
            local.to_str().unwrap(),
            remote.to_str().unwrap(),
            download.to_str().unwrap(),
        );
        sftp.upload(local, remote, false).await.unwrap();
        assert_eq!(tokio::fs::read(remote).await.unwrap(), data);
        sftp.download(remote, download, false).await.unwrap();
        assert_eq!(tokio::fs::read(download).await.unwrap(), data);
        tokio::fs::write(remote, &data[..12345]).await.unwrap();
        sftp.upload(local, remote, true).await.unwrap();
        assert_eq!(tokio::fs::read(remote).await.unwrap(), data);
        tokio::fs::write(download, &data[..54321]).await.unwrap();
        sftp.download(remote, download, true).await.unwrap();
        assert_eq!(tokio::fs::read(download).await.unwrap(), data);
        tokio::fs::write(local, []).await.unwrap();
        sftp.upload(local, remote, false).await.unwrap();
        sftp.download(remote, download, false).await.unwrap();
        assert!(tokio::fs::read(download).await.unwrap().is_empty());
        assert!(sftp
            .download("/aetherdesk-does-not-exist", download, false)
            .await
            .is_err());
        drop(sftp);
        proxy.abort();
        child.kill().await.unwrap();
    }
}
