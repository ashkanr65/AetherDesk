//! Own each connection independently. No network operation runs on the UI thread.
use ad_desktop_model::profiles::{ConnectionType, SavedSession};
use ad_ssh::{FileProgress, RemoteFileRequest, RemoteFileResult, SshClient, SshConfig};
use eframe::egui;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{runtime::Runtime, sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

pub struct Credentials {
    pub username: String,
    pub secret: String,
    pub confirmed_fingerprint: Option<String>,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyChallenge {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
}
impl HostKeyChallenge {
    pub fn parse(message: &str, host: &str, port: u16) -> Option<Self> {
        let (_, tail) = message.split_once("AETHERDESK_HOST_KEY_UNKNOWN|")?;
        let values: Vec<_> = tail.trim().split('|').collect();
        if values.len() != 4 || values[0] != host || values[1].parse::<u16>().ok()? != port {
            return None;
        }
        let fingerprint = values[3];
        if !fingerprint.starts_with("SHA256:")
            || fingerprint.len() > 100
            || fingerprint.chars().any(char::is_whitespace)
        {
            return None;
        }
        Some(Self {
            host: host.into(), port, algorithm: values[2].into(), fingerprint: fingerprint.into(),
        })
    }
}

pub enum Event {
    Connected,
    Output(Vec<u8>),
    TrustRequired(HostKeyChallenge),
    Failed(String),
    Closed,
    Files { request: u64, result: Result<RemoteFileResult, String> },
}
pub enum Command {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Files { id: u64, request: RemoteFileRequest, progress: Arc<FileProgress> },
}

pub struct Connection {
    commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    cancel: CancellationToken,
}
impl Connection {
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands.try_send(command).map_err(|_| {
            "The connection is busy or closed. The action was not queued.".into()
        })
    }
    pub fn disconnect(&self) { self.cancel.cancel(); }
}
impl Drop for Connection {
    fn drop(&mut self) { self.cancel.cancel(); }
}

/// The runtime outlives tab close operations so SSH cleanup can finish.
pub struct Connections {
    runtime: Option<Runtime>,
    cancel: CancellationToken,
}
impl Connections {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            runtime: Some(tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2).thread_name("aetherdesk-native").enable_all()
                .build().map_err(|e| e.to_string())?),
            cancel: CancellationToken::new(),
        })
    }
    pub fn open(&self, profile: &SavedSession, credentials: &Credentials, ctx: egui::Context) -> Result<Connection, String> {
        let config = connection_config(profile, credentials)?;
        let shell = profile.connection_type == ConnectionType::SshTerminal;
        let (send, receive) = mpsc::channel(64);
        let (events, output) = mpsc::channel(128);
        let cancel = self.cancel.child_token();
        let worker_cancel = cancel.clone();
        self.runtime.as_ref().ok_or("Connection runtime is closed")?.spawn(async move {
            run(config, shell, receive, EventSink { sender: events, ctx }, worker_cancel).await;
        });
        Ok(Connection { commands: send, events: output, cancel })
    }
    pub fn list_local(&self, path: PathBuf, ctx: egui::Context) -> mpsc::Receiver<Result<Vec<LocalEntry>, String>> {
        let (send, receive) = mpsc::channel(1);
        if let Some(runtime) = &self.runtime {
            runtime.spawn_blocking(move || {
                let result = local_entries(&path).map_err(|e| e.to_string());
                let _ = send.blocking_send(result);
                ctx.request_repaint();
            });
        }
        receive
    }
}
impl Drop for Connections {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(runtime) = self.runtime.take() {
            // Never block the native window's destruction on network cleanup.
            // Keep the executor alive briefly so cancellation can close channels.
            std::thread::spawn(move || runtime.shutdown_timeout(Duration::from_secs(3)));
        }
    }
}

fn connection_config(profile: &SavedSession, credentials: &Credentials) -> Result<SshConfig, String> {
    if !matches!(profile.connection_type, ConnectionType::SshTerminal | ConnectionType::SftpOnly) {
        return Err("This connection type is not connected to the native interface yet.".into());
    }
    if profile.host.trim().is_empty() || profile.host.chars().any(char::is_control)
        || credentials.username.trim().is_empty() || credentials.username.contains('\0') {
        return Err("Enter a host and username without control characters.".into());
    }
    let port = u16::try_from(profile.port).ok().filter(|p| *p != 0).ok_or("Port must be between 1 and 65535")?;
    let mut config = SshConfig {
        host: profile.host.trim().into(), port, username: credentials.username.clone(),
        wireguard_config_path: profile.wireguard_config_path.clone().filter(|p| !p.is_empty()),
        confirmed_host_fingerprint: credentials.confirmed_fingerprint.clone(),
        trust_new_host_key: credentials.confirmed_fingerprint.is_some(),
        ..Default::default()
    };
    match profile.authentication.as_str() {
        "password" => config.password = Some(credentials.secret.clone()),
        "sshKey" => {
            config.key_path = Some(profile.key_path.clone().filter(|s| !s.trim().is_empty()).ok_or("Select a private key file")?);
            config.key_passphrase = (!credentials.secret.is_empty()).then(|| credentials.secret.clone());
        }
        _ => return Err("This authentication method is not supported by the original SSH backend.".into()),
    }
    Ok(config)
}

struct EventSink { sender: mpsc::Sender<Event>, ctx: egui::Context }
impl EventSink {
    fn send(&self, event: Event) -> bool {
        // Bounded output prevents a background tab consuming unlimited memory.
        let sent = self.sender.try_send(event).is_ok();
        self.ctx.request_repaint();
        sent
    }
}

async fn run(config: SshConfig, shell: bool, mut commands: mpsc::Receiver<Command>, events: EventSink, cancel: CancellationToken) {
    let host = config.host.clone();
    let port = config.port;
    let client = SshClient::standalone(config);
    let connect = async {
        let mut session = client.connect().await.map_err(|e| e.to_string())?;
        if shell {
            if let Err(error) = session.open_shell().await {
                let _ = session.close().await;
                return Err(error.to_string());
            }
        }
        Ok::<_, String>(session)
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => { events.send(Event::Closed); return; },
        result = tokio::time::timeout(Duration::from_secs(75), connect) => result.unwrap_or_else(|_| Err("SSH authentication or setup timed out".into())),
    };
    let session = match result {
        Ok(value) => value,
        Err(error) => {
            if let Some(challenge) = HostKeyChallenge::parse(&error, &host, port) {
                events.send(Event::TrustRequired(challenge));
            } else { events.send(Event::Failed(error)); }
            return;
        }
    };
    drop(client); // Authentication data does not survive the connection attempt.
    if cancel.is_cancelled() || !events.send(Event::Connected) {
        let _ = session.close().await;
        return;
    }
    let mut jobs = JoinSet::new();
    let mut progress = Vec::<Arc<FileProgress>>::new();
    let mut interval = tokio::time::interval(Duration::from_millis(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            command = commands.recv() => match command {
                Some(Command::Input(bytes)) => if let Err(e) = session.send_data(&bytes) { events.send(Event::Failed(e.to_string())); break; },
                Some(Command::Resize { cols, rows }) => if let Err(e) = session.resize_pty(cols, rows) { events.send(Event::Failed(e.to_string())); break; },
                Some(Command::Files { id, request, progress: item }) => {
                    if jobs.len() >= 4 {
                        events.send(Event::Files { request: id, result: Err("Four file operations are already running. Wait or cancel one.".into()) });
                        continue;
                    }
                    progress.retain(|p| Arc::strong_count(p) > 1);
                    progress.push(item.clone());
                    let session = session.clone();
                    jobs.spawn(async move { (id, session.file_operation(request, item).await.map_err(|e| e.to_string())) });
                }
                None => break,
            },
            Some(result) = jobs.join_next(), if !jobs.is_empty() => match result {
                Ok((id, result)) => { if !events.send(Event::Files { request: id, result }) { break; } },
                Err(_) => { events.send(Event::Failed("A file operation stopped unexpectedly".into())); break; },
            },
            _ = interval.tick(), if shell => match session.read_available(32768) {
                Ok(bytes) if !bytes.is_empty() => if !events.send(Event::Output(bytes)) { break; },
                Ok(_) => {},
                Err(error) => { events.send(Event::Failed(error.to_string())); break; },
            },
        }
    }
    for item in progress { item.cancel(); }
    // Let staged saves finish their commit/cleanup before closing the transport.
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while jobs.join_next().await.is_some() {}
    }).await;
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    let _ = session.close().await;
    events.send(Event::Closed);
}

#[derive(Clone)]
pub struct LocalEntry { pub path: PathBuf, pub name: String, pub directory: bool, pub size: u64 }
fn local_entries(path: &std::path::Path) -> std::io::Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        entries.push(LocalEntry { path: entry.path(), name: entry.file_name().to_string_lossy().into_owned(), directory: metadata.is_dir(), size: metadata.len() });
    }
    entries.sort_by_key(|v| (!v.directory, v.name.to_lowercase()));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_cannot_target_another_host_or_port() {
        let message = "SSH connection failed: AETHERDESK_HOST_KEY_UNKNOWN|host|22|ssh-ed25519|SHA256:abc";
        assert!(HostKeyChallenge::parse(message, "host", 22).is_some());
        assert!(HostKeyChallenge::parse(message, "other", 22).is_none());
        assert!(HostKeyChallenge::parse(message, "host", 23).is_none());
        assert!(HostKeyChallenge::parse(&(message.to_owned() + "|extra"), "host", 22).is_none());
    }
    #[test]
    fn invalid_ports_and_unavailable_auth_do_not_silently_fallback() {
        let mut profile = SavedSession::from_json(&serde_json::json!({"id":"a","name":"A","host":"host"})).unwrap();
        let credentials = Credentials { username:"me".into(), secret:"secret".into(), confirmed_fingerprint:None };
        profile.port = 65536;
        assert!(connection_config(&profile, &credentials).is_err());
        profile.port = 22;
        profile.authentication = "mfa".into();
        assert!(connection_config(&profile, &credentials).is_err());
        profile.authentication = "password".into();
        let config = connection_config(&profile, &credentials).unwrap();
        assert!(!config.trust_new_host_key);
        assert!(config.confirmed_host_fingerprint.is_none());
    }
    #[test]
    fn closing_a_tab_cancels_its_worker_even_with_a_full_command_queue() {
        let (commands, _receive) = mpsc::channel(1);
        commands.try_send(Command::Input(vec![1])).unwrap_or_else(|_| panic!("queue"));
        let (_send, events) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let connection = Connection { commands, events, cancel:cancel.clone() };
        drop(connection);
        assert!(cancel.is_cancelled());
    }
}
