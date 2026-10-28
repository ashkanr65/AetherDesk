//! Own each connection independently. No network operation runs on the UI thread.
const SHELL_INTEGRATION: &str = r#"printf '\r\033[2K'; __aetherdesk_cd(){ if [ -n "${2-}" ]; then printf '\033]777;%s;begin\007' "$2"; fi; cd -- "$1"; __ad_cd_status=$?; if [ -n "${2-}" ]; then printf '\033]777;%s;end\007' "$2"; fi; return "$__ad_cd_status"; }; if [ -n "${BASH_VERSION:-}" ]; then case ":${HISTIGNORE-}:" in *": __aetherdesk_cd *:"*) ;; *) HISTIGNORE="${HISTIGNORE:+$HISTIGNORE:} __aetherdesk_cd *";; esac; __aetherdesk_emit_cwd(){ local __ad_status=$? __ad_path=$PWD; __ad_path=${__ad_path//\%/%25}; __ad_path=${__ad_path//;/%3B}; __ad_path=${__ad_path//\?/%3F}; __ad_path=${__ad_path//#/%23}; printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$__ad_path"; return "$__ad_status"; }; case ";${PROMPT_COMMAND[*]:-};" in *';__aetherdesk_emit_cwd;'*) ;; *) if declare -p PROMPT_COMMAND 2>/dev/null | grep -q '^declare -a'; then PROMPT_COMMAND=(__aetherdesk_emit_cwd "${PROMPT_COMMAND[@]}"); else PROMPT_COMMAND="__aetherdesk_emit_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; fi ;; esac; __aetherdesk_emit_cwd; fi"#;
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
            host: host.into(),
            port,
            algorithm: values[2].into(),
            fingerprint: fingerprint.into(),
        })
    }
}

pub enum Event {
    Connected,
    Authenticated(Arc<ad_ssh::SshSession>),
    Desktop(Arc<crate::remote::RemoteClient>),
    DesktopEnded,
    Output(Vec<u8>),
    X11Unavailable(String),
    X11Restored,
    TrustRequired(HostKeyChallenge),
    Failed(String),
    Closed,
    Files {
        request: u64,
        result: Result<RemoteFileResult, String>,
    },
}
pub enum Command {
    RestartX11,
    EndDesktop,
    Input(Vec<u8>),
    Navigate(String),
    CancelNavigation,
    Resize {
        cols: u16,
        rows: u16,
    },
    Files {
        id: u64,
        request: RemoteFileRequest,
        progress: Arc<FileProgress>,
    },
}

pub struct Connection {
    clock: crate::server_clock::Clock,
    commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
    cancel: CancellationToken,
    finished: Arc<std::sync::atomic::AtomicBool>,
}
impl Connection {
    pub fn server_time(&self) -> String {
        self.clock
            .text(!self.cancel.is_cancelled() && !self.is_finished())
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| "The connection is busy or closed. The action was not queued.".into())
    }
    pub fn disconnect(&self) {
        self.cancel.cancel();
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Acquire)
    }
}
struct Finished(Arc<std::sync::atomic::AtomicBool>, egui::Context);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
        self.1.request_repaint();
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The runtime outlives tab close operations so SSH cleanup can finish.
pub struct Connections {
    runtime: Option<Runtime>,
    x11: Arc<crate::x11::X11Runtime>,
    cancel: CancellationToken,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}
impl Connections {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            runtime: Some(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("aetherdesk-native")
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?,
            ),
            cancel: CancellationToken::new(),
            x11: Arc::new(crate::x11::X11Runtime::default()),
            tasks: std::sync::Mutex::new(Vec::new()),
        })
    }
    pub fn open(
        &self,
        profile: &SavedSession,
        credentials: &Credentials,
        ctx: egui::Context,
        desktop_id: uuid::Uuid,
    ) -> Result<Connection, String> {
        if self.cancel.is_cancelled() {
            return Err("AetherDesk is closing".into());
        }
        let config = connection_config(profile, credentials)?;
        let kind = profile.connection_type;
        let environment = profile.desktop_environment.clone();
        let (send, receive) = mpsc::channel(64);
        let (events, output) = mpsc::channel(128);
        let cancel = self.cancel.child_token();
        let worker_cancel = cancel.clone();
        let clock = crate::server_clock::Clock::default();
        let worker_clock = clock.clone();
        let x11 = self.x11.clone();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finish_guard = Finished(finished.clone(), ctx.clone());
        let task = self
            .runtime
            .as_ref()
            .ok_or("Connection runtime is closed")?
            .spawn(async move {
                let _finished = finish_guard;
                run(
                    config,
                    kind,
                    environment,
                    desktop_id,
                    receive,
                    EventSink {
                        sender: events,
                        ctx,
                    },
                    worker_cancel,
                    x11,
                    worker_clock,
                )
                .await;
            });
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
        Ok(Connection {
            clock,
            commands: send,
            events: output,
            cancel,
            finished,
        })
    }
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(runtime) = &self.runtime {
            let task = runtime.spawn(future);
            let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
    }
    pub fn begin_shutdown(&self) {
        self.cancel.cancel();
    }
    pub fn is_idle(&self) -> bool {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .all(|task| task.is_finished())
    }
    pub fn list_local(
        &self,
        path: PathBuf,
        ctx: egui::Context,
    ) -> mpsc::Receiver<Result<Vec<LocalEntry>, String>> {
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
            // Main window defers closure until connection cleanup completes.
            // Exceptional destruction must still never block the UI thread.
            runtime.shutdown_background();
        }
    }
}

fn connection_config(
    profile: &SavedSession,
    credentials: &Credentials,
) -> Result<SshConfig, String> {
    if profile.host.trim().is_empty()
        || profile.host.chars().any(char::is_control)
        || credentials.username.trim().is_empty()
        || credentials.username.chars().any(char::is_control)
    {
        return Err("Enter a host and username without control characters.".into());
    }
    let port = u16::try_from(profile.port)
        .ok()
        .filter(|p| *p != 0)
        .ok_or("Port must be between 1 and 65535")?;
    let mut config = SshConfig {
        host: profile.host.trim().into(),
        port,
        username: credentials.username.clone(),
        wireguard_config_path: profile
            .wireguard_config_path
            .clone()
            .filter(|p| profile.wireguard_enabled && !p.is_empty()),
        confirmed_host_fingerprint: credentials.confirmed_fingerprint.clone(),
        trust_new_host_key: credentials.confirmed_fingerprint.is_some(),
        ..Default::default()
    };
    match profile.authentication.as_str() {
        "password" => config.password = Some(credentials.secret.clone()),
        "sshKey" => {
            config.key_path = Some(
                profile
                    .key_path
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or("Select a private key file")?,
            );
            config.key_passphrase =
                (!credentials.secret.is_empty()).then(|| credentials.secret.clone());
        }
        _ => {
            return Err(
                "This authentication method is not supported by the original SSH backend.".into(),
            )
        }
    }
    Ok(config)
}

struct EventSink {
    sender: mpsc::Sender<Event>,
    ctx: egui::Context,
}
impl EventSink {
    fn send(&self, event: Event) -> bool {
        // Bounded output prevents a background tab consuming unlimited memory.
        let sent = self.sender.try_send(event).is_ok();
        self.ctx.request_repaint();
        sent
    }
}

async fn run(
    mut config: SshConfig,
    kind: ConnectionType,
    desktop_environment: String,
    desktop_id: uuid::Uuid,
    mut commands: mpsc::Receiver<Command>,
    events: EventSink,
    cancel: CancellationToken,
    x11_runtime: Arc<crate::x11::X11Runtime>,
    clock: crate::server_clock::Clock,
) {
    let shell = matches!(kind, ConnectionType::SshTerminal | ConnectionType::SshX11);
    let mut x11 = if kind == ConnectionType::SshX11 {
        let result = x11_runtime.acquire(&cancel).await;
        match result {
            Ok(server) => Some(server),
            Err(error) => {
                if error == "Connection cancelled" {
                    events.send(Event::Closed);
                } else {
                    events.send(Event::Failed(error));
                }
                return;
            }
        }
    } else {
        None
    };
    if let Some(server) = &x11 {
        config.x11 = Some(server.config.clone());
    }
    let host = config.host.clone();
    let port = config.port;
    let x11_config = config.x11.clone();
    let client = SshClient::standalone(config);
    let connect = async {
        let result = if matches!(kind, ConnectionType::RemoteDesktop | ConnectionType::SshX11) {
            client.connect_with_bulk_transport().await
        } else {
            client.connect().await
        };
        let mut session = result.map_err(|e| e.to_string())?;
        if shell {
            if let Err(error) = session.open_shell_with_bootstrap(SHELL_INTEGRATION).await {
                let _ = session.close().await;
                return Err(error.to_string());
            }
        }
        Ok::<_, String>(session)
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            if let Some(server) = &mut x11 { server.stop().await; }
            events.send(Event::Closed); return;
        },
        result = tokio::time::timeout(Duration::from_secs(crate::preferences::current().connect_timeout), connect) => result.unwrap_or_else(|_| Err("SSH authentication or setup timed out".into())),
    };
    let session = match result {
        Ok(value) => value,
        Err(error) => {
            if let Some(server) = &mut x11 {
                server.stop().await;
            }
            if let Some(challenge) = HostKeyChallenge::parse(&error, &host, port) {
                events.send(Event::TrustRequired(challenge));
            } else {
                events.send(Event::Failed(error));
            }
            return;
        }
    };
    drop(client); // Authentication data does not survive the connection attempt.
    let mut desktop = if kind == ConnectionType::RemoteDesktop {
        let result = crate::remote::RemoteClient::start(
            Arc::new(session.clone()),
            desktop_environment,
            desktop_id,
            events.ctx.clone(),
            &cancel,
        )
        .await;
        match result {
            Ok(remote) => {
                events.send(Event::Desktop(remote.clone()));
                Some(remote)
            }
            Err(error) => {
                let _ = session.close().await;
                if error == "Connection cancelled" {
                    events.send(Event::Closed);
                } else {
                    events.send(Event::Failed(error));
                }
                return;
            }
        }
    } else {
        None
    };
    if cancel.is_cancelled() || !events.send(Event::Connected) {
        if let Some(remote) = &desktop {
            if let Err(error) = remote
                .stop(cancel.is_cancelled() && remote.newly_created())
                .await
            {
                events.send(Event::Failed(error));
            }
        }
        let _ = session.close().await;
        if let Some(server) = &mut x11 {
            server.stop().await;
        }
        events.send(Event::Closed);
        return;
    }
    events.send(Event::Authenticated(Arc::new(session.clone())));
    let _clock =
        crate::server_clock::Lease::start(session.clone(), clock, events.ctx.clone(), &cancel);
    let mut navigation = crate::terminal_navigation::Navigation::default();
    let mut jobs = JoinSet::new();
    let mut progress = Vec::<Arc<FileProgress>>::new();
    let mut interval = tokio::time::interval(Duration::from_millis(if shell { 10 } else { 250 }));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut x11_check = tokio::time::interval(Duration::from_millis(500));
    x11_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut x11_check_warned = false;
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            command = commands.recv() => match command {
                Some(Command::RestartX11) => {
                    if let Some(config) = &x11_config {
                        match x11_runtime.recover(config, &cancel).await {
                            Ok(lease) => { x11 = Some(lease); x11_check_warned = false; events.send(Event::X11Restored); },
                            Err(error) => { events.send(Event::X11Unavailable(error)); },
                        }
                    }
                },
                Some(Command::EndDesktop) => {
                    if let Some(remote)=desktop.take() {
                        match remote.stop(true).await {
                            Ok(())=>{events.send(Event::DesktopEnded);},
                            Err(error)=>{events.send(Event::Failed(error));},
                        }
                        break;
                    }
                },
                Some(Command::Input(bytes)) => if let Err(e) = session.send_data(&bytes) { events.send(Event::Failed(e.to_string())); break; },
                Some(Command::Navigate(path)) => { if shell { navigation.request(path); } },
                Some(Command::CancelNavigation) => navigation.cancel_queued(),
                Some(Command::Resize { cols, rows }) => if let Err(e) = session.resize_pty(cols, rows) { events.send(Event::Failed(e.to_string())); break; },
                Some(Command::Files { id, request, progress: item }) => {
                    if jobs.len() >= 4 {
                        events.send(Event::Files { request: id, result: Err("Four file operations are already running. Wait or cancel one.".into()) });
                        continue;
                    }
                    progress.retain(|p| Arc::strong_count(p) > 1);
                    progress.push(item.clone());
                    let session = session.clone();
                    jobs.spawn(async move { (id, file_operation(&session, request, item).await) });
                }
                None => break,
            },
            Some(result) = jobs.join_next(), if !jobs.is_empty() => match result {
                Ok((id, result)) => { if !events.send(Event::Files { request: id, result }) { break; } },
                Err(_) => { events.send(Event::Failed("A file operation stopped unexpectedly".into())); break; },
            },
            _ = x11_check.tick(), if x11.is_some() => {
                match x11.as_ref().expect("X11 lease").failure() {
                    Ok(Some(failure)) => {
                        events.send(Event::X11Unavailable(failure.message()));
                        if let Some(mut stopped) = x11.take() { stopped.stop().await; }
                    },
                    Ok(None) => {},
                    Err(error) if !x11_check_warned => {
                        x11_check_warned = true;
                        tracing::warn!(%error, "could not inspect the X11 process; SSH was preserved");
                    },
                    Err(_) => {},
                }
            },
            _ = interval.tick() => {
                if let Some(remote)=&desktop {
                    let state=remote.peek_status();
                    if state.state!="connected" {events.send(Event::Failed(state.error.unwrap_or_else(||"Remote desktop closed".into())));break;}
                }
                if !shell && session.is_closed() {events.send(Event::Failed("The SSH transport closed".into()));break;}
                if shell {match session.read_available(32768) {
                    Ok(bytes) if !bytes.is_empty() => {
                        let bytes = navigation.filter(&bytes);
                        if !bytes.is_empty() && !events.send(Event::Output(bytes)) { break; }
                    },
                    Ok(_) => {},
                    Err(error) => { events.send(Event::Failed(error.to_string())); break; },
                }}
                let expired = navigation.expire(std::time::Instant::now());
                if !expired.is_empty() && !events.send(Event::Output(expired)) { break; }
                if shell { if let Some(command) = navigation.command() {
                    if let Err(error) = session.send_data(&command) { events.send(Event::Failed(error.to_string())); break; }
                }}
            },
        }
    }
    for item in progress {
        item.cancel();
    }
    // Let staged saves finish their commit/cleanup before closing the transport.
    let cleanup = tokio::time::timeout(Duration::from_secs(12), async {
        while jobs.join_next().await.is_some() {}
    })
    .await;
    if cleanup.is_err() {
        events.send(Event::Failed("Disconnected; a file operation's final result could not be confirmed. Check its destination before retrying.".into()));
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    if let Some(remote) = desktop {
        if let Err(error) = remote.stop(false).await {
            events.send(Event::Failed(error));
        }
    }
    let _ = session.close().await;
    if let Some(server) = &mut x11 {
        server.stop().await;
    }
    events.send(Event::Closed);
}

async fn file_operation(
    session: &ad_ssh::SshSession,
    request: RemoteFileRequest,
    progress: Arc<FileProgress>,
) -> Result<RemoteFileResult, String> {
    if request.recursive && matches!(request.operation.as_str(), "upload" | "download") {
        return folder_transfer(session, request, progress).await;
    }
    file_operation_one(session, request, progress).await
}
async fn file_operation_one(
    session: &ad_ssh::SshSession,
    mut request: RemoteFileRequest,
    progress: Arc<FileProgress>,
) -> Result<RemoteFileResult, String> {
    if matches!(request.operation.as_str(), "rename" | "delete") {
        let metadata = session
            .file_operation(
                transfer_request("stat", request.path.clone(), None),
                progress.clone(),
            )
            .await
            .map_err(|e| e.to_string())?
            .metadata
            .ok_or("File metadata missing")?;
        crate::file_workspace::check_remote(session, &metadata.canonical_path)?;
    }
    if request.operation != "download" {
        return session
            .file_operation(request, progress)
            .await
            .map_err(|e| e.to_string());
    }
    let destination = PathBuf::from(
        request
            .local_path
            .as_ref()
            .ok_or("Choose a local destination")?,
    );
    let parent = destination
        .parent()
        .ok_or("Choose a destination directory")?
        .to_path_buf();
    let stage = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".aetherdesk-download-")
            .tempfile_in(parent)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    request.local_path = Some(
        stage
            .path()
            .to_str()
            .ok_or("Local destination is not Unicode")?
            .into(),
    );
    let result = session
        .file_operation(request, progress)
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || publish_download(stage, destination))
        .await
        .map_err(|e| e.to_string())??;
    Ok(result)
}

fn transfer_request(operation: &str, path: String, local: Option<String>) -> RemoteFileRequest {
    RemoteFileRequest {
        operation: operation.into(),
        path,
        local_path: local,
        destination: None,
        expected_revision: None,
        permissions: None,
        overwrite: false,
        recursive: false,
    }
}
fn child_name(name: &str) -> Result<(), String> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err("Invalid child name in folder transfer".into());
    }
    Ok(())
}
async fn folder_transfer(
    session: &ad_ssh::SshSession,
    request: RemoteFileRequest,
    progress: Arc<FileProgress>,
) -> Result<RemoteFileResult, String> {
    let local = PathBuf::from(request.local_path.as_ref().ok_or("Choose a local folder")?);
    if request.operation == "upload" {
        let local = tokio::fs::canonicalize(local)
            .await
            .map_err(|e| e.to_string())?;
        let mut stack = vec![(local, request.path.clone(), 0usize)];
        let mut count = 0usize;
        while let Some((local, remote, depth)) = stack.pop() {
            if depth > 128 || count > 10000 {
                return Err("Folder transfer exceeds 128 levels or 10,000 entries; completed files remain on the server".into());
            }
            session
                .file_operation(
                    transfer_request("mkdir", remote.clone(), None),
                    progress.clone(),
                )
                .await
                .map_err(|e| {
                    format!("Folder copy stopped: {e}. Completed files remain on the server.")
                })?;
            let mut entries = tokio::fs::read_dir(local)
                .await
                .map_err(|e| e.to_string())?;
            while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                count += 1;
                if count > 10000 {
                    return Err("Folder transfer entry limit reached".into());
                }
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "Filename is not Unicode")?;
                child_name(&name)?;
                let target = format!("{}/{name}", remote.trim_end_matches('/'));
                let kind = entry.file_type().await.map_err(|e| e.to_string())?;
                let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err("Folder copy does not follow Windows junctions or links".into());
                    }
                }
                if kind.is_symlink() {
                    return Err("Folder copy does not follow symbolic links".into());
                }
                if kind.is_dir() {
                    stack.push((entry.path(), target, depth + 1));
                } else if kind.is_file() {
                    file_operation_one(
                        session,
                        transfer_request(
                            "upload",
                            target,
                            Some(entry.path().to_string_lossy().into_owned()),
                        ),
                        progress.clone(),
                    )
                    .await?;
                } else {
                    return Err("Folder contains an unsupported special file".into());
                }
            }
        }
    } else {
        if local.exists() {
            return Err(
                "Choose a new destination folder; existing folders are never replaced".into(),
            );
        }
        let stage = tempfile::Builder::new()
            .prefix(".aetherdesk-folder-")
            .tempdir_in(local.parent().ok_or("Local folder parent missing")?)
            .map_err(|e| e.to_string())?;
        let mut stack = vec![(request.path.clone(), stage.path().to_path_buf(), 0usize)];
        let mut count = 0usize;
        while let Some((remote, local, depth)) = stack.pop() {
            if depth > 128 {
                return Err("Folder transfer exceeds 128 levels".into());
            }
            let entries = session
                .file_operation(transfer_request("list", remote, None), progress.clone())
                .await
                .map_err(|e| e.to_string())?
                .entries
                .unwrap_or_default();
            for entry in entries {
                if matches!(entry.name.as_str(), "." | "..") {
                    continue;
                }
                count += 1;
                if count > 10000 {
                    return Err("Folder transfer exceeds 10,000 entries".into());
                }
                child_name(&entry.name)?;
                let name = ad_desktop_model::files::safe_local_name(&entry.name);
                let target = local.join(name);
                match entry.kind.as_str() {
                    "directory" => {
                        tokio::fs::create_dir(&target)
                            .await
                            .map_err(|e| e.to_string())?;
                        stack.push((entry.path, target, depth + 1));
                    }
                    "file" => {
                        file_operation_one(
                            session,
                            transfer_request(
                                "download",
                                entry.path,
                                Some(target.to_string_lossy().into_owned()),
                            ),
                            progress.clone(),
                        )
                        .await?;
                    }
                    _ => {
                        return Err(
                            "Folder copy does not follow symbolic links or special files".into(),
                        )
                    }
                }
            }
        }
        // On Windows, rename of a directory fails if the destination exists.
        // Publish only after every child has downloaded and passed revision checks.
        if local.exists() {
            return Err("The destination appeared during transfer; it was not replaced".into());
        }
        std::fs::rename(stage.path(), &local).map_err(|e| e.to_string())?;
    }
    session
        .file_operation(transfer_request("stat", request.path, None), progress)
        .await
        .map_err(|e| e.to_string())
}

fn publish_download(stage: tempfile::NamedTempFile, destination: PathBuf) -> Result<(), String> {
    stage.as_file().sync_all().map_err(|e| e.to_string())?;
    stage
        .persist_noclobber(destination)
        .map_err(|e| e.error.to_string())?;
    Ok(())
}

#[derive(Clone)]
pub struct LocalEntry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub size: u64,
}
fn local_entries(path: &std::path::Path) -> std::io::Result<Vec<LocalEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        entries.push(LocalEntry {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            directory: metadata.is_dir(),
            size: metadata.len(),
        });
    }
    entries.sort_by_key(|v| (!v.directory, v.name.to_lowercase()));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wireguard_opt_out_uses_direct_transport_without_changing_other_tab_profiles() {
        let mut profile=SavedSession::from_json(&serde_json::json!({"id":"vpn","name":"VPN","host":"server","wireguardConfigPath":"C:/vpn/test.conf"})).unwrap();
        let credentials = Credentials {
            username: "fixture".into(),
            secret: String::new(),
            confirmed_fingerprint: None,
        };
        let sibling = profile.clone();
        profile.wireguard_enabled = false;
        assert!(connection_config(&profile, &credentials)
            .unwrap()
            .wireguard_config_path
            .is_none());
        assert_eq!(
            connection_config(&sibling, &credentials)
                .unwrap()
                .wireguard_config_path
                .as_deref(),
            Some("C:/vpn/test.conf")
        );
    }
    #[test]
    fn downloaded_file_is_published_without_overwriting_an_existing_target() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("target");
        std::fs::write(&destination, b"original").unwrap();
        let mut stage = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        stage.write_all(b"incoming").unwrap();
        assert!(publish_download(stage, destination.clone()).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        let mut stage = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
        stage.write_all(b"incoming").unwrap();
        let new_destination = directory.path().join("new-file");
        publish_download(stage, new_destination.clone()).unwrap();
        assert_eq!(std::fs::read(new_destination).unwrap(), b"incoming");
    }
    #[test]
    fn confirmation_cannot_target_another_host_or_port() {
        let message =
            "SSH connection failed: AETHERDESK_HOST_KEY_UNKNOWN|host|22|ssh-ed25519|SHA256:abc";
        assert!(HostKeyChallenge::parse(message, "host", 22).is_some());
        assert!(HostKeyChallenge::parse(message, "other", 22).is_none());
        assert!(HostKeyChallenge::parse(message, "host", 23).is_none());
        assert!(HostKeyChallenge::parse(&(message.to_owned() + "|extra"), "host", 22).is_none());
    }
    #[test]
    fn invalid_ports_and_unavailable_auth_do_not_silently_fallback() {
        let mut profile =
            SavedSession::from_json(&serde_json::json!({"id":"a","name":"A","host":"host"}))
                .unwrap();
        let credentials = Credentials {
            username: "me".into(),
            secret: "secret".into(),
            confirmed_fingerprint: None,
        };
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
        commands
            .try_send(Command::Input(vec![1]))
            .unwrap_or_else(|_| panic!("queue"));
        let (_send, events) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let connection = Connection {
            clock: crate::server_clock::Clock::default(),
            commands,
            events,
            cancel: cancel.clone(),
            finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        drop(connection);
        assert!(cancel.is_cancelled());
    }
    #[test]
    fn worker_completion_is_reported_even_when_its_task_is_dropped() {
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Finished(finished.clone(), egui::Context::default());
        assert!(!finished.load(std::sync::atomic::Ordering::Acquire));
        drop(guard);
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
    }
}
