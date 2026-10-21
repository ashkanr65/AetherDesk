use crate::{
    connection::{
        Command, Connection, Connections, Credentials, Event, HostKeyChallenge, LocalEntry,
    },
    terminal::Terminal,
};
use ad_desktop_model::profiles::{ConnectionType, SavedSession};
use ad_ssh::{FileProgress, RemoteFileRequest, SftpEntry};
use eframe::egui;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
};
use tokio::sync::mpsc;
use zeroize::Zeroize;

enum Pending {
    List { path: String, generation: u64 },
    Operation(Arc<FileProgress>),
}
pub struct SessionTab {
    tab_id: uuid::Uuid,
    notices: std::collections::VecDeque<crate::notifications::Notice>,
    failed: bool,
    pub profile: SavedSession,
    pub connected: bool,
    backend: Option<Arc<ad_ssh::SshSession>>,
    pub pending_open: Vec<crate::editor::OpenFile>,
    desktop_id: uuid::Uuid,
    remote: Option<crate::remote_view::RemoteView>,
    connecting: bool,
    connection: Option<Connection>,
    credentials: Credentials,
    master_password: String,
    credentials_open: bool,
    credential_attempted: bool,
    credential_connect_after: bool,
    credential_loaded: bool,
    remember_changed: bool,
    credential_profile: Option<SavedSession>,
    profile_update: Option<SavedSession>,
    credential_reply: Option<mpsc::Receiver<Result<Option<zeroize::Zeroizing<String>>, String>>>,
    challenge: Option<HostKeyChallenge>,
    status: String,
    terminal: Terminal,
    pty_size: Option<(u16, u16)>,
    files_visible: bool,
    files_local_pane: bool,
    sync_directories: bool,
    local_path: String,
    local_loaded_path: String,
    local_entries: Vec<LocalEntry>,
    local_pending: Option<(String, mpsc::Receiver<Result<Vec<LocalEntry>, String>>)>,
    remote_path: String,
    remote_loaded_path: String,
    remote_entries: Vec<SftpEntry>,
    pending: HashMap<u64, Pending>,
    serial: u64,
    list_generation: u64,
    action: Option<FileAction>,
    property_text: Option<String>,
    local_action_reply: Option<mpsc::Receiver<Result<Option<String>, String>>>,
    compare_remote: bool,
    selected_local: Option<String>,
    selected_remote: Option<String>,
    replace_transfers: bool,
    transfer_generation: u64,
    explorer_hover: Option<egui::Pos2>,
    navigation_generation: Option<u64>,
    sync_generation: Option<u64>,
}
struct FileAction {
    operation: String,
    path: String,
    value: String,
    directory: bool,
}
#[derive(Clone)]
struct FileDrag {
    connection: String,
    local: bool,
    path: String,
    name: String,
    directory: bool,
}
impl SessionTab {
    pub fn new(profile: SavedSession) -> Self {
        let files_visible = profile.connection_type == ConnectionType::SftpOnly;
        let status = if profile.remember_password {
            "Enter a password or unlock the saved credential to connect."
        } else {
            "Enter credentials to connect. Passwords are kept in memory for this attempt only."
        }
        .into();
        let desktop_id = retained_desktop_id(&profile.id);
        Self {
            tab_id: uuid::Uuid::new_v4(),
            notices: std::collections::VecDeque::new(),
            failed: false,
            credentials: Credentials {
                username: profile.username.clone(),
                secret: String::new(),
                confirmed_fingerprint: None,
            },
            profile,
            master_password: String::new(),
            credentials_open: true,
            credential_attempted: false,
            credential_connect_after: false,
            credential_loaded: false,
            remember_changed: false,
            credential_profile: None,
            profile_update: None,
            credential_reply: None,
            connected: false,
            backend: None,
            pending_open: Vec::new(),
            desktop_id,
            remote: None,
            connecting: false,
            connection: None,
            challenge: None,
            status,
            terminal: Terminal::default(),
            pty_size: None,
            files_visible,
            files_local_pane: false,
            sync_directories: crate::preferences::current()
                .follow_cwd
                .unwrap_or_else(|| crate::terminal_settings::current().1.follow_cwd),
            local_path: std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()),
            local_loaded_path: String::new(),
            local_entries: vec![],
            local_pending: None,
            remote_path: ".".into(),
            remote_loaded_path: String::new(),
            remote_entries: vec![],
            pending: HashMap::new(),
            serial: 0,
            list_generation: 0,
            action: None,
            property_text: None,
            local_action_reply: None,
            compare_remote: false,
            selected_local: None,
            selected_remote: None,
            replace_transfers: false,
            transfer_generation: 0,
            explorer_hover: None,
            navigation_generation: None,
            sync_generation: None,
        }
    }
    pub fn prefill_credentials(&mut self, secret: String, master: String) {
        self.credentials.secret.zeroize();
        self.master_password.zeroize();
        // The form calls this only after its credential/profile save succeeds.
        self.credential_loaded = self.profile.remember_password && !secret.is_empty();
        self.credentials.secret = secret;
        self.master_password = master;
    }
    pub fn restore_disconnected(&mut self) {
        self.credentials_open = false;
    }
    pub fn take_profile_update(&mut self) -> Option<SavedSession> {
        self.profile_update.take()
    }
    fn open_credentials(&mut self) {
        if self.credential_reply.is_none() {
            self.credential_attempted = false;
            self.credential_loaded = false;
            self.credential_connect_after = false;
        }
        self.credentials_open = true;
    }
    fn connect(&mut self, runtime: &Connections, ctx: &egui::Context) {
        if self.credential_reply.is_some() {
            return;
        }
        if self.profile.remember_password
            && self.credentials.secret.is_empty()
            && !self.credential_attempted
        {
            if self.profile.master_password_protected && self.master_password.is_empty() {
                self.status = "Enter the master password to unlock the saved credential".into();
                self.credentials_open = true;
                return;
            }
            self.credential_connect_after = true;
            self.credential_operation(false, runtime, ctx);
            return;
        }
        if self.connection.as_ref().is_some_and(|c| !c.is_finished()) {
            self.status =
                "Finishing the previous connection. Please wait before reconnecting.".into();
            return;
        }
        if self.profile.connection_type == ConnectionType::RemoteDesktop
            && self.desktop_id != retained_desktop_id(&self.profile.id)
        {
            if let Err(error) = crate::desktop_recovery::remember(
                &self.profile,
                &self.credentials.username,
                self.desktop_id,
            ) {
                self.status = error;
                return;
            }
        }
        match runtime.open(
            &self.profile,
            &self.credentials,
            ctx.clone(),
            self.desktop_id,
        ) {
            Ok(connection) => {
                self.remote = None;
                self.backend = None;
                self.connection = Some(connection);
                self.connecting = true;
                self.failed = false;
                self.connected = false;
                self.challenge = None;
                self.status = "Connecting…".into();
                self.pending.clear();
                self.terminal = Terminal::default();
                self.pty_size = None;
                self.remote_entries.clear();
                self.remote_loaded_path.clear();
            }
            Err(error) => self.error(error),
        }
    }
    pub fn poll(&mut self, runtime: &Connections, ctx: &egui::Context) {
        let generation = crate::transfers::generation();
        if generation != self.transfer_generation {
            self.transfer_generation = generation;
            if self.connected {
                self.refresh_remote();
                self.refresh_local(runtime, ctx);
            }
        }
        if let Some(reply) = &mut self.local_action_reply {
            if let Ok(result) = reply.try_recv() {
                self.local_action_reply = None;
                match result {
                    Ok(Some(text)) => self.property_text = Some(text),
                    Ok(None) => {
                        self.status = "File operation completed".into();
                        self.refresh_local(runtime, ctx);
                    }
                    Err(error) => self.error(error),
                }
            }
        }
        if let Some(receiver) = &mut self.credential_reply {
            match receiver.try_recv() {
                Ok(result) => {
                    self.credential_reply = None;
                    let connect_after = std::mem::take(&mut self.credential_connect_after);
                    match result {
                        Ok(Some(mut secret)) => {
                            self.credentials.secret.zeroize();
                            self.credentials.secret = std::mem::take(&mut *secret);
                            self.status = "Saved credential unlocked".into();
                            self.credential_loaded = true;
                            if connect_after {
                                self.connect(runtime, ctx);
                            }
                        }
                        Ok(None) => {
                            self.profile_update = self.credential_profile.take();
                            if let Some(profile) = &self.profile_update {
                                self.profile = profile.clone();
                            }
                            self.remember_changed = false;
                            if self.connected {
                                self.status = if self.profile.remember_password {
                                    "Connected · password saved"
                                } else {
                                    "Connected · password saving off"
                                }
                                .into();
                            }
                        }
                        Err(error) => {
                            self.credential_profile = None;
                            self.status = error.clone();
                            self.notify(crate::notifications::Kind::Error, error);
                            if connect_after {
                                self.credentials_open = true;
                            }
                        }
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.credential_reply = None;
                    self.credential_connect_after = false;
                    self.credential_profile = None;
                    self.status = "Credential operation did not complete".into();
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(30))
                }
            }
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut events = Vec::new();
        if let Some(connection) = &mut self.connection {
            while let Ok(event) = connection.events.try_recv() {
                events.push(event);
            }
            // If the UI event queue filled, the worker closes instead of losing
            // terminal bytes silently. Surface the closed worker as a failure.
            if connection.events.is_closed()
                && connection.events.is_empty()
                && events.is_empty()
                && (self.connected || self.connecting)
            {
                events.push(Event::Failed(
                    "The connection worker stopped. Reconnect to continue.".into(),
                ));
            }
        }
        for event in events {
            match event {
                Event::Desktop(client) => {
                    self.remote =
                        Some(crate::remote_view::RemoteView::new(client, self.desktop_id));
                }
                Event::DesktopEnded => {
                    self.status = "Remote session ended successfully".into();
                    self.notify(crate::notifications::Kind::Status, self.status.clone());
                    self.connected = false;
                    self.connecting = false;
                    self.remote = None;
                    if self.desktop_id != retained_desktop_id(&self.profile.id) {
                        if let Err(error) = crate::desktop_recovery::forget(self.desktop_id) {
                            self.status = format!("Remote session ended successfully. Recovery entry could not be removed: {error}");
                        }
                    }
                }
                Event::Authenticated(session) => {
                    self.backend = Some(session);
                }
                Event::Connected => {
                    self.credentials_open = false;
                    self.connected = true;
                    self.connecting = false;
                    self.status = "Connected".into();
                    self.notify(crate::notifications::Kind::Status, "Connected".into());
                    self.authenticated_credentials(runtime, ctx);
                    self.credentials.secret.zeroize();
                    self.master_password.zeroize();
                    self.credentials.confirmed_fingerprint = None;
                    self.refresh_remote();
                    self.refresh_local(runtime, ctx);
                }
                Event::Output(bytes) => {
                    let replies = self.terminal.process(&bytes);
                    if !replies.is_empty() {
                        self.send(Command::Input(replies));
                    }
                    if let Some(directory) = self.pending_shell_directory() {
                        self.remote_path = directory;
                        self.refresh_remote();
                        self.sync_generation = Some(self.list_generation);
                    }
                }
                Event::TrustRequired(challenge) => {
                    self.challenge = Some(challenge);
                    self.connecting = false;
                    self.connected = false;
                    self.status = "Host identity confirmation required".into();
                    self.notify(crate::notifications::Kind::Warning, self.status.clone());
                }
                Event::Failed(error) => {
                    self.credentials_open = true;
                    self.master_password.zeroize();
                    self.remote = None;
                    self.backend = None;
                    self.error(error);
                    self.failed = true;
                    self.connected = false;
                    self.connecting = false;
                    self.credentials.secret.zeroize();
                    self.credentials.confirmed_fingerprint = None;
                    self.pending.clear();
                }
                Event::Closed => {
                    self.remote = None;
                    self.backend = None;
                    if self.connected || self.connecting {
                        self.status = "Disconnected successfully".into();
                        self.notify(crate::notifications::Kind::Status, self.status.clone());
                    }
                    self.connected = false;
                    self.connecting = false;
                    self.credentials.secret.zeroize();
                    self.pending.clear();
                }
                Event::Files { request, result } => {
                    let Some(pending) = self.pending.remove(&request) else {
                        continue;
                    };
                    match (pending, result) {
                        (Pending::List { path, generation }, Ok(result))
                            if generation == self.list_generation =>
                        {
                            let path = result
                                .metadata
                                .as_ref()
                                .map(|m| m.canonical_path.clone())
                                .unwrap_or(path);
                            self.remote_loaded_path = path.clone();
                            self.remote_path = path;
                            match self.pending_terminal_navigation(generation) {
                                Ok(Some(path)) => {
                                    self.send(Command::Navigate(path));
                                }
                                Ok(None) => {}
                                Err(error) => self.status = error.into(),
                            }
                            self.remote_entries = result.entries.unwrap_or_default();
                            self.remote_entries
                                .retain(|entry| !matches!(entry.name.as_str(), "." | ".."));
                            self.remote_entries
                                .sort_by_key(|v| (v.kind != "directory", v.name.to_lowercase()));
                        }
                        (Pending::List { generation, .. }, _)
                            if generation != self.list_generation => {}
                        (Pending::Operation(_), Ok(result)) => {
                            self.status = "File operation completed".into();
                            self.notify(crate::notifications::Kind::Status, self.status.clone());
                            if let Some(metadata) = result.metadata {
                                self.property_text = serde_json::to_string_pretty(&metadata).ok();
                            }
                            self.refresh_remote();
                            self.refresh_local(runtime, ctx);
                        }
                        (_, Err(error)) => self.error(error),
                        _ => {}
                    }
                }
            }
        }
        if let Some((path, receiver)) = &mut self.local_pending {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok(entries) => {
                        self.local_entries = entries;
                        self.local_loaded_path = path.clone();
                        self.local_path = path.clone();
                    }
                    Err(error) => self.status = error,
                }
                self.local_pending = None;
            }
        }
    }
    fn send(&mut self, command: Command) -> bool {
        if let Some(connection) = &self.connection {
            match connection.send(command) {
                Ok(()) => return true,
                Err(error) => self.status = error,
            }
        }
        false
    }
    fn refresh_local(&mut self, runtime: &Connections, ctx: &egui::Context) {
        self.local_pending = Some((
            self.local_path.clone(),
            runtime.list_local(PathBuf::from(&self.local_path), ctx.clone()),
        ));
    }
    fn refresh_remote(&mut self) {
        self.sync_generation = None;
        if !self.connected {
            return;
        }
        self.serial += 1;
        self.list_generation += 1;
        let request = request("list", self.remote_path.clone());
        let id = self.serial;
        if let Some(connection) = &self.connection {
            match connection.send(Command::Files {
                id,
                request,
                progress: Arc::new(FileProgress::default()),
            }) {
                Ok(()) => {
                    self.pending.insert(
                        id,
                        Pending::List {
                            path: self.remote_path.clone(),
                            generation: self.list_generation,
                        },
                    );
                }
                Err(error) => self.status = error,
            }
        }
    }
    fn operation(&mut self, request: RemoteFileRequest) {
        if matches!(request.operation.as_str(), "upload" | "download") {
            if let (Some(session), Some(local)) = (&self.backend, request.local_path) {
                self.status = match crate::transfers::enqueue(
                    session,
                    request.operation == "upload",
                    local,
                    request.path,
                    request.recursive,
                    request.overwrite,
                ) {
                    Ok(()) => "Transfer queued. Use Pause / Resume to retain partial data.".into(),
                    Err(error) => error,
                };
            } else {
                self.status = "Connect and choose a local path before transferring".into();
            }
            return;
        }
        self.serial += 1;
        let progress = Arc::new(FileProgress::default());
        if let Some(connection) = &self.connection {
            match connection.send(Command::Files {
                id: self.serial,
                request,
                progress: progress.clone(),
            }) {
                Ok(()) => {
                    self.pending
                        .insert(self.serial, Pending::Operation(progress));
                    self.status = "Working…".into();
                }
                Err(error) => self.status = error,
            }
        }
    }
    pub fn ai_workspace(&self) -> Option<crate::ai_tools::Workspace> {
        if !self.connected {
            return None;
        }
        let session = self.backend.as_ref()?.clone();
        Some(crate::ai_tools::Workspace::Ssh {
            session: session.clone(),
            root: if self.remote_loaded_path.is_empty() {
                ".".into()
            } else {
                self.remote_loaded_path.clone()
            },
            label: self.profile.name.clone(),
            id: format!("{:?}", session.connection_id()),
        })
    }
    pub fn ssh_session(&self) -> Option<Arc<ad_ssh::SshSession>> {
        if self.connected {
            self.backend.clone()
        } else {
            None
        }
    }
    pub fn capture(&self) -> Option<crate::remote::Image> {
        self.remote.as_ref()?.client.frame()
    }
    pub fn log_source(&self) -> Option<(Arc<ad_ssh::SshSession>, uuid::Uuid)> {
        if self.connected && self.remote.is_some() {
            Some((self.backend.as_ref()?.clone(), self.desktop_id))
        } else {
            None
        }
    }
    pub fn identity(&self) -> String {
        match &self.backend {
            Some(session) if self.connected => format!("{:?}", session.connection_id()),
            _ => format!("tab:{}", self.desktop_id),
        }
    }
    pub fn terminal_text(&self) -> Result<String, String> {
        if !matches!(
            self.profile.connection_type,
            ConnectionType::SshTerminal | ConnectionType::SshX11
        ) {
            return Err("This connection does not have a terminal".into());
        }
        Ok(self.terminal.visible_text())
    }
    pub fn open_remote_file(&mut self, path: String) -> Result<(), String> {
        let session = self
            .backend
            .as_ref()
            .filter(|s| self.connected && !s.is_closed())
            .ok_or("The connection is closed")?
            .clone();
        self.pending_open.push(crate::editor::OpenFile {
            location: crate::editor::Location::Remote { session, path },
            force_text: false,
            action: crate::editor::OpenAction::Default,
        });
        Ok(())
    }
    pub fn open_remote_folder(&mut self, path: String) -> Result<(), String> {
        if !self.connected {
            return Err("The connection is closed".into());
        }
        self.remote_path = path;
        self.files_visible = true;
        self.refresh_remote();
        Ok(())
    }
    pub fn deactivate(&mut self) {
        let release = self.terminal.release_mouse();
        if self.connected && !release.is_empty() {
            self.send(Command::Input(release));
        }
        if let Some(remote) = &mut self.remote {
            remote.release();
        }
    }
    pub fn tab_id(&self) -> uuid::Uuid {
        self.tab_id
    }
    pub fn state_label(&self) -> &'static str {
        if self.connected {
            "Connected"
        } else if self.connecting {
            "Connecting"
        } else if self.challenge.is_some() {
            "Confirm host"
        } else if self.failed {
            "Failed"
        } else {
            "Disconnected"
        }
    }
    pub fn is_active(&self) -> bool {
        self.connected || self.connecting
    }
    pub fn take_notices(&mut self) -> impl Iterator<Item = crate::notifications::Notice> + '_ {
        self.notices.drain(..)
    }
    fn notify(&mut self, kind: crate::notifications::Kind, text: String) {
        if self.notices.len() >= 256 {
            self.notices.pop_front();
        }
        self.notices.push_back(crate::notifications::Notice::new(
            kind,
            format!("{}: {text}", self.profile.name),
        ));
    }
    fn error(&mut self, error: String) {
        self.status = error.clone();
        self.notify(crate::notifications::Kind::Error, error);
    }
    pub fn disconnect(&mut self) {
        self.deactivate();
        self.credentials.secret.zeroize();
        self.challenge = None;
        if let Some(connection) = &self.connection {
            connection.disconnect();
        }
        self.status = "Disconnecting…".into();
    }
    pub fn make_independent_desktop(&mut self) {
        self.desktop_id = uuid::Uuid::new_v4();
    }
    pub fn desktop_identity(&self) -> uuid::Uuid {
        self.desktop_id
    }
    pub fn recover(profile: SavedSession, desktop_id: uuid::Uuid, username: String) -> Self {
        let mut tab = Self::new(profile);
        tab.desktop_id = desktop_id;
        tab.credentials.username = username;
        tab.status = "Enter credentials to reopen this saved desktop tab".into();
        tab
    }
    fn authenticated_credentials(&mut self, runtime: &Connections, ctx: &egui::Context) {
        if self.remember_changed || (self.profile.remember_password && !self.credential_loaded) {
            self.credential_operation(true, runtime, ctx);
        } else if self.profile.remember_password {
            let mut profile = self.profile.clone();
            profile.username = self.credentials.username.clone();
            profile.remember_username = true;
            self.profile = profile.clone();
            self.profile_update = Some(profile);
        }
    }
    fn credential_operation(&mut self, save: bool, runtime: &Connections, ctx: &egui::Context) {
        let mut profile = self.profile.clone();
        let username = self.credentials.username.clone();
        if save {
            profile.username = username.clone();
            if profile.remember_password {
                profile.remember_username = true;
            } else {
                profile.master_password_protected = false;
            }
            self.credential_profile = Some(profile.clone());
        } else {
            self.credential_attempted = true;
        }
        let secret = zeroize::Zeroizing::new(self.credentials.secret.clone());
        let master = zeroize::Zeroizing::new(self.master_password.clone());
        let (sender, receiver) = mpsc::channel(1);
        self.credential_reply = Some(receiver);
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                if save {
                    if profile.remember_password {
                        crate::credentials::remember(&profile, &username, &secret, &master)
                            .map(|_| None)
                    } else {
                        crate::vault::delete(&format!("ssh-{}", profile.id)).map(|_| None)
                    }
                } else {
                    crate::credentials::recall(&profile, &username, &master).and_then(|v| {
                        v.map(|secret| Some(zeroize::Zeroizing::new(secret)))
                            .ok_or_else(|| "No saved credential for this connection".into())
                    })
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|v| v);
            let _ = sender.send(result).await;
            ctx.request_repaint();
        });
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, runtime: &Connections) {
        let ctx = ui.ctx().clone();
        if !self.connected && !self.connecting {
            if self.profile.remember_password
                && !self.profile.master_password_protected
                && self.credentials.username == self.profile.username
                && self.credentials.secret.is_empty()
                && !self.credential_attempted
                && self.credential_reply.is_none()
                && self.credentials_open
            {
                self.credential_operation(false, runtime, &ctx);
            }
            if self.credentials_open && self.challenge.is_none() {
                let mut open = true;
                let mut connect = false;
                egui::Window::new(format!("Connect · {}", self.profile.name))
                    .id(egui::Id::new(("session-credentials", self.tab_id)))
                    .open(&mut open)
                    .default_width(520.0)
                    .resizable(false)
                    .show(&ctx, |ui| {
                        let enter = crate::dialog::primary(ui);
                        ui.label(format!("{}:{}", self.profile.host, self.profile.port));
                        egui::Grid::new(("credentials", self.tab_id))
                            .num_columns(2)
                            .spacing([14.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("Username");
                                if ui
                                    .add_enabled(
                                        self.credential_reply.is_none(),
                                        egui::TextEdit::singleline(&mut self.credentials.username),
                                    )
                                    .changed()
                                {
                                    if self.credential_loaded {
                                        self.credentials.secret.zeroize();
                                    }
                                    self.credential_loaded = false;
                                    self.credential_attempted = false;
                                }
                                ui.end_row();
                                ui.label(if self.profile.authentication == "sshKey" {
                                    "Key passphrase"
                                } else {
                                    "Password"
                                });
                                if ui
                                    .add_enabled(
                                        self.credential_reply.is_none(),
                                        egui::TextEdit::singleline(&mut self.credentials.secret)
                                            .password(true),
                                    )
                                    .changed()
                                {
                                    self.credential_loaded = false;
                                }
                                ui.end_row();
                                if self.profile.remember_password
                                    && self.profile.master_password_protected
                                {
                                    ui.label("Master password");
                                    if ui
                                        .add_enabled(
                                            self.credential_reply.is_none(),
                                            egui::TextEdit::singleline(&mut self.master_password)
                                                .password(true),
                                        )
                                        .changed()
                                    {
                                        self.credential_attempted = false;
                                    }
                                    ui.end_row();
                                }
                            });
                        if ui
                            .add_enabled(
                                self.credential_reply.is_none(),
                                egui::Checkbox::new(
                                    &mut self.profile.remember_password,
                                    "Save password / passphrase securely for reconnects",
                                ),
                            )
                            .changed()
                        {
                            self.remember_changed = true;
                            if self.profile.remember_password {
                                self.profile.remember_username = true;
                            }
                        }
                        ui.horizontal(|ui| {
                            if self.profile.remember_password {
                                if ui
                                    .add_enabled(
                                        self.credential_reply.is_none(),
                                        egui::Button::new("Unlock saved credential"),
                                    )
                                    .clicked()
                                {
                                    self.credential_operation(false, runtime, &ctx);
                                }
                                if ui
                                    .add_enabled(
                                        self.credential_reply.is_none(),
                                        egui::Button::new("Forget saved credential"),
                                    )
                                    .clicked()
                                {
                                    match crate::vault::delete(&format!("ssh-{}", self.profile.id))
                                    {
                                        Ok(()) => {
                                            self.credential_loaded = false;
                                            self.credentials.secret.zeroize();
                                            self.master_password.zeroize();
                                            self.status = "Saved credential removed".into();
                                        }
                                        Err(error) => self.status = error,
                                    }
                                }
                            }
                            let can_connect = self.credential_reply.is_none()
                                && self.connection.as_ref().is_none_or(|c| c.is_finished());
                            let response =
                                ui.add_enabled(can_connect, egui::Button::new("Connect"));
                            crate::dialog::focus_on_open(ui, &response);
                            connect = response.clicked() || (enter && can_connect);
                        });
                        ui.label(&self.status);
                    });
                self.credentials_open = open;
                if connect {
                    self.credentials_open = false;
                    self.connect(runtime, &ctx);
                } else if !open {
                    self.credential_reply = None;
                    self.credential_connect_after = false;
                    self.credentials.secret.zeroize();
                    self.master_password.zeroize();
                }
            }
        }
        egui::ScrollArea::horizontal()
            .id_salt(("session-controls", self.tab_id))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if !self.connected && !self.connecting && ui.button("Connect…").clicked() {
                        self.open_credentials();
                    }
                    if (self.connected || self.connecting)
                        && crate::ui_icons::button(
                            ui,
                            crate::ui_icons::Icon::Disconnect,
                            if self.connecting {
                                "Cancel connection"
                            } else if self.profile.connection_type == ConnectionType::RemoteDesktop
                            {
                                "Disconnect (keep remote desktop session)"
                            } else {
                                "Disconnect"
                            },
                            false,
                        )
                        .clicked()
                    {
                        self.disconnect();
                    }
                    if self.connected
                        && self.profile.connection_type == ConnectionType::RemoteDesktop
                    {
                        if crate::ui_icons::button(
                            ui,
                            crate::ui_icons::Icon::EndSession,
                            "End remote session",
                            false,
                        )
                        .clicked()
                        {
                            self.end_desktop();
                        }
                    }
                    if self.profile.connection_type != ConnectionType::SftpOnly {
                        if crate::ui_icons::button(
                            ui,
                            crate::ui_icons::Icon::Folder,
                            "Open or close file transfer",
                            self.files_visible,
                        )
                        .clicked()
                        {
                            if self.files_visible {
                                self.close_files();
                            } else {
                                self.files_visible = true;
                            }
                        }
                    }
                    if matches!(
                        self.profile.connection_type,
                        ConnectionType::SshTerminal | ConnectionType::SshX11
                    ) {
                        self.terminal.settings_button(ui);
                    }
                    if let Some(remote) = &mut self.remote {
                        remote.controls(ui);
                    }
                    ui.add(egui::Label::new(&self.status).truncate())
                        .on_hover_text(&self.status);
                });
            });
        if let Some(challenge) = self.challenge.clone() {
            egui::Window::new("Verify SSH host")
                .id(egui::Id::new(("verify-host", self.tab_id)))
                .resizable(false)
                .show(&ctx, |ui| {
                    ui.label(format!(
                        "Unknown SSH host: {}:{}",
                        challenge.host, challenge.port
                    ));
                    ui.label(format!(
                        "{} · {}",
                        challenge.algorithm, challenge.fingerprint
                    ));
                    ui.label("Verify this fingerprint with the server owner before trusting it.");
                    ui.horizontal(|ui| {
                        if ui.button("Trust this key and connect").clicked() {
                            self.credentials.confirmed_fingerprint =
                                Some(challenge.fingerprint.clone());
                            self.connect(runtime, &ctx);
                        }
                        if ui.button("Cancel").clicked() {
                            self.credentials.secret.zeroize();
                            self.master_password.zeroize();
                            self.credentials.confirmed_fingerprint = None;
                            self.challenge = None;
                            self.status = "Connection cancelled".into();
                        }
                    });
                });
        }
        if self.profile.connection_type == ConnectionType::SftpOnly && self.connected {
            ui.push_id(("files", self.tab_id), |ui| self.files_ui(ui, runtime));
        }
        if matches!(
            self.profile.connection_type,
            ConnectionType::SshTerminal | ConnectionType::SshX11
        ) {
            let action = self.terminal.ui(ui, self.connected);
            if self.connected {
                let size = self.terminal.size();
                if self.pty_size != Some(size) {
                    let (cols, rows) = size;
                    if self.send(Command::Resize { cols, rows }) {
                        self.pty_size = Some(size);
                    }
                }
                if !action.input.is_empty() {
                    self.send(Command::Input(action.input));
                }
            }
        }
        let desktop_action = self.remote.as_mut().and_then(|remote| {
            remote.ui(ui);
            remote.take_action()
        });
        match desktop_action {
            Some(crate::remote_view::SessionAction::Disconnect) => self.disconnect(),
            Some(crate::remote_view::SessionAction::End) => self.end_desktop(),
            None => {}
        }
        if !self.files_visible || self.profile.connection_type == ConnectionType::SftpOnly {
            self.file_dialogs(&ctx, runtime);
        }
    }
    fn file_dialogs(&mut self, ctx: &egui::Context, runtime: &Connections) {
        self.action_ui(ctx, runtime);
        if let Some(text) = self.property_text.clone() {
            let mut open = true;
            egui::Window::new("File properties")
                .id(egui::Id::new(("properties", self.tab_id)))
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&text).monospace()).selectable(true),
                    );
                });
            if !open {
                self.property_text = None;
            }
        }
    }
    fn end_desktop(&mut self) {
        if let Some(remote) = &mut self.remote {
            remote.release();
        }
        if self.send(Command::EndDesktop) {
            self.status = "Ending remote session…".into();
        }
    }
    pub fn captures_remote_keyboard(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.captures_keyboard())
    }
    fn pending_shell_directory(&mut self) -> Option<String> {
        self.terminal
            .take_directory()
            .filter(|directory| self.sync_directories && directory != &self.remote_loaded_path)
    }
    fn pending_terminal_navigation(
        &mut self,
        generation: u64,
    ) -> Result<Option<String>, &'static str> {
        if self.navigation_generation.take() == Some(generation)
            && self.sync_directories
            && matches!(
                self.profile.connection_type,
                ConnectionType::SshTerminal | ConnectionType::SshX11
            )
        {
            ad_desktop_model::files::quote_terminal_path(&self.remote_loaded_path).map(Some)
        } else {
            Ok(None)
        }
    }
    fn set_directory_sync(&mut self, enabled: bool) {
        self.sync_directories = enabled;
        self.navigation_generation = None;
        if !enabled {
            if self.sync_generation.take() == Some(self.list_generation) {
                self.list_generation += 1;
                self.remote_path = self.remote_loaded_path.clone();
            }
            self.send(Command::CancelNavigation);
        }
    }
    fn directory_sync_ui(&mut self, ui: &mut egui::Ui) {
        if matches!(
            self.profile.connection_type,
            ConnectionType::SshTerminal | ConnectionType::SshX11
        ) {
            let mut enabled = self.sync_directories;
            if ui.checkbox(&mut enabled, "Synchronize terminal and file folders")
                .on_hover_text("When enabled, navigating either the terminal or server files changes the other. When disabled, both stay independent.").changed() {
                self.set_directory_sync(enabled);
            }
            ui.separator();
        }
    }
    fn close_files(&mut self) {
        self.files_visible = false;
        self.action = None;
        self.property_text = None;
    }
    pub(crate) fn files_viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("session-files", self.tab_id))
    }
    pub(crate) fn diagnostic_files_open(&self) -> bool {
        self.files_visible
    }
    /// Only used by the explicitly isolated UI self-check. No network is opened.
    pub(crate) fn diagnostic_files(directory: &std::path::Path) -> Self {
        let mut profile = crate::profile_store::new_profile();
        profile.host = "fixture.invalid".into();
        profile.name = "Isolated file-window check".into();
        profile.connection_type = ConnectionType::SshX11;
        let mut tab = Self::new(profile);
        tab.connected = true;
        tab.files_visible = true;
        tab.credentials_open = false;
        tab.sync_directories = false;
        tab.local_path = directory.display().to_string();
        tab.local_loaded_path = tab.local_path.clone();
        tab.remote_path = "/fixture".into();
        tab.remote_loaded_path = tab.remote_path.clone();
        tab.local_entries = vec![LocalEntry {
            path: directory.join("local-fixture"),
            name: "local-fixture".into(),
            directory: true,
            size: 0,
        }];
        tab.remote_entries = vec![SftpEntry {
            path: "/fixture/server-fixture".into(),
            name: "server-fixture".into(),
            kind: "directory".into(),
            size: None,
            permissions: None,
            modified: None,
        }];
        tab
    }
    pub fn files_window(&mut self, ctx: &egui::Context, runtime: &Connections) {
        if !self.files_visible || self.profile.connection_type == ConnectionType::SftpOnly {
            return;
        }
        let id = self.files_viewport_id();
        let builder = crate::app_layout::builder(
            ctx,
            id,
            format!("Files · {}", self.profile.name),
            egui::vec2(1100.0, 650.0),
        );
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            crate::app_layout::update(ctx, "files");
            if ctx.input(|i| i.viewport().close_requested()) {
                self.close_files();
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Close);
                return;
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                if self.connected {
                    ui.push_id(("files", self.tab_id), |ui| self.files_ui(ui, runtime));
                } else {
                    ui.label("This connection is disconnected. Reconnect to browse server files.");
                }
            });
            self.file_dialogs(ctx, runtime);
        });
    }
    fn files_ui(&mut self, ui: &mut egui::Ui, runtime: &Connections) {
        self.directory_sync_ui(ui);
        let ctx = ui.ctx().clone();
        let mut local_go = false;
        let mut remote_go = false;
        let mut next_local = None;
        let mut next_remote = None;
        let mut new_action = None;
        let mut open_files = Vec::new();
        let mut insert_path = None;
        let narrow = files_narrow(
            ui.available_width(),
            ui.text_style_height(&egui::TextStyle::Body),
        );
        if narrow {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.files_local_pane, true, "This computer");
                ui.selectable_value(&mut self.files_local_pane, false, "Server");
            });
        }
        let show_local = !narrow || self.files_local_pane;
        let show_remote = !narrow || !self.files_local_pane;
        // Bound each child to the remaining client area. Its contents cannot
        // enlarge the native window or spill across the other pane on a DPI change.
        let available = ui.available_rect_before_wrap().intersect(ui.clip_rect());
        let bounds =
            egui::Rect::from_min_size(available.min, available.size().max(egui::Vec2::ZERO));
        let rects = file_pane_rects(bounds, narrow, ui.spacing().item_spacing.x);
        let mut columns = [
            ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("local-pane")
                    .max_rect(rects[0])
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            ),
            ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("remote-pane")
                    .max_rect(rects[1])
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            ),
        ];
        for (column, rect) in columns.iter_mut().zip(rects) {
            column.set_clip_rect(rect.intersect(ui.clip_rect()));
        }
        {
            if show_local {
                columns[0].label("This computer");
                let path_size = [
                    columns[0].available_width(),
                    columns[0].spacing().interact_size.y,
                ];
                let response = columns[0]
                    .add_sized(path_size, egui::TextEdit::singleline(&mut self.local_path));
                local_go |= crate::dialog::field_submit(&columns[0], &response);
                columns[0].horizontal_wrapped(|ui| {
                    local_go |= ui.button("Go").clicked();
                    if ui.button("↑").clicked() {
                        next_local = PathBuf::from(&self.local_loaded_path)
                            .parent()
                            .map(|p| p.to_string_lossy().into_owned());
                    }
                    if ui.button("New folder").clicked() {
                        new_action = Some(FileAction {
                            operation: "local_mkdir".into(),
                            path: String::new(),
                            value: PathBuf::from(&self.local_loaded_path)
                                .join("new-folder")
                                .display()
                                .to_string(),
                            directory: true,
                        });
                    }
                });
                let row_height = (columns[0].text_style_height(&egui::TextStyle::Button)
                    + 2.0 * columns[0].spacing().button_padding.y)
                    .max(columns[0].spacing().interact_size.y);
                egui::ScrollArea::vertical()
                    .id_salt("local-files")
                    .max_height((columns[0].available_height() - 8.0).max(1.0))
                    .show_rows(
                        &mut columns[0],
                        row_height,
                        self.local_entries.len(),
                        |ui, range| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            for entry in &self.local_entries[range] {
                                let label = if entry.directory {
                                    entry.name.clone()
                                } else {
                                    format!("{}  ({} B)", entry.name, entry.size)
                                };
                                let response = crate::ui_icons::file_entry(
                                    ui,
                                    entry.directory,
                                    &label,
                                    self.selected_local.as_deref() == entry.path.to_str(),
                                );
                                let response = response.interact(egui::Sense::click_and_drag());
                                response.dnd_set_drag_payload(FileDrag {
                                    connection: self.identity(),
                                    local: true,
                                    path: entry.path.to_string_lossy().into_owned(),
                                    name: entry.name.clone(),
                                    directory: entry.directory,
                                });
                                if response.clicked() {
                                    self.selected_local = Some(entry.path.display().to_string());
                                }
                                if response.clicked_by(egui::PointerButton::Middle) {
                                    insert_path = Some(entry.path.display().to_string());
                                }
                                if response.double_clicked() && !entry.directory {
                                    open_files.push(crate::editor::OpenFile {
                                        location: crate::editor::Location::Local(
                                            entry.path.clone(),
                                        ),
                                        force_text: false,
                                        action: crate::editor::OpenAction::Default,
                                    });
                                }
                                if response.double_clicked() && entry.directory {
                                    next_local = Some(entry.path.to_string_lossy().into_owned());
                                }
                                response.context_menu(|ui| {
                                    if ui.button("Open").clicked() {
                                        if entry.directory {
                                            next_local = Some(entry.path.display().to_string());
                                        } else {
                                            open_files.push(crate::editor::OpenFile {
                                                location: crate::editor::Location::Local(
                                                    entry.path.clone(),
                                                ),
                                                force_text: false,
                                                action: crate::editor::OpenAction::Default,
                                            });
                                        }
                                        ui.close();
                                    }
                                    if !entry.directory {
                                        for (label, action) in [
                                            (
                                                "Open with…",
                                                crate::editor::OpenAction::ChooseProgram,
                                            ),
                                            (
                                                "Open with default program",
                                                crate::editor::OpenAction::DefaultProgram,
                                            ),
                                        ] {
                                            if ui.button(label).clicked() {
                                                open_files.push(crate::editor::OpenFile {
                                                    location: crate::editor::Location::Local(
                                                        entry.path.clone(),
                                                    ),
                                                    force_text: false,
                                                    action,
                                                });
                                                ui.close();
                                            }
                                        }
                                    }
                                    for (label, operation) in [
                                        ("Rename…", "local_rename"),
                                        ("Delete…", "local_delete"),
                                        ("Properties", "local_properties"),
                                        ("Permissions…", "local_permissions"),
                                    ] {
                                        if ui.button(label).clicked() {
                                            new_action = Some(FileAction {
                                                operation: operation.into(),
                                                path: entry.path.display().to_string(),
                                                value: entry.path.display().to_string(),
                                                directory: entry.directory,
                                            });
                                            ui.close();
                                        }
                                    }
                                    if !entry.directory && ui.button("Compare file with…").clicked()
                                    {
                                        new_action = Some(FileAction {
                                            operation: "local_compare".into(),
                                            path: entry.path.display().to_string(),
                                            value: self
                                                .selected_local
                                                .clone()
                                                .filter(|p| p != &entry.path.display().to_string())
                                                .unwrap_or_default(),
                                            directory: false,
                                        });
                                        ui.close();
                                    }
                                    if ui.button("Copy file path to terminal").clicked() {
                                        insert_path = Some(entry.path.display().to_string());
                                        ui.close();
                                    }
                                    if !entry.directory
                                        && ui.button("Open with text editor").clicked()
                                    {
                                        open_files.push(crate::editor::OpenFile {
                                            location: crate::editor::Location::Local(
                                                entry.path.clone(),
                                            ),
                                            force_text: true,
                                            action: crate::editor::OpenAction::Default,
                                        });
                                        ui.close();
                                    }
                                    if ui.button("Copy file path").clicked() {
                                        ui.ctx()
                                            .copy_text(entry.path.to_string_lossy().into_owned());
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            !self.remote_loaded_path.is_empty(),
                                            egui::Button::new("Copy to server…"),
                                        )
                                        .clicked()
                                    {
                                        new_action = Some(FileAction {
                                            operation: "upload".into(),
                                            path: entry.path.to_string_lossy().into_owned(),
                                            value: join_remote(
                                                &self.remote_loaded_path,
                                                &entry.name,
                                            ),
                                            directory: entry.directory,
                                        });
                                        ui.close();
                                    }
                                });
                            }
                        },
                    );
            }
            if show_remote {
                columns[1].label("Server");
                let path_size = [
                    columns[1].available_width(),
                    columns[1].spacing().interact_size.y,
                ];
                let response = columns[1]
                    .add_sized(path_size, egui::TextEdit::singleline(&mut self.remote_path));
                remote_go |= crate::dialog::field_submit(&columns[1], &response);
                columns[1].horizontal_wrapped(|ui| {
                    remote_go |= ui.button("Go").clicked();
                    if ui.button("↑").clicked() {
                        next_remote = Some(remote_parent(&self.remote_loaded_path));
                    }
                    if ui
                        .add_enabled(
                            !self.remote_loaded_path.is_empty(),
                            egui::Button::new("New folder"),
                        )
                        .clicked()
                    {
                        new_action = Some(FileAction {
                            operation: "mkdir".into(),
                            path: String::new(),
                            value: join_remote(&self.remote_loaded_path, "new-folder"),
                            directory: true,
                        });
                    }
                });
                let row_height = (columns[1].text_style_height(&egui::TextStyle::Button)
                    + 2.0 * columns[1].spacing().button_padding.y)
                    .max(columns[1].spacing().interact_size.y);
                egui::ScrollArea::vertical()
                    .id_salt("remote-files")
                    .max_height((columns[1].available_height() - 8.0).max(1.0))
                    .show_rows(
                        &mut columns[1],
                        row_height,
                        self.remote_entries.len(),
                        |ui, range| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            for entry in &self.remote_entries[range] {
                                let directory = entry.kind == "directory";
                                let response = crate::ui_icons::file_entry(
                                    ui,
                                    directory,
                                    &entry.name,
                                    self.selected_remote.as_deref() == Some(entry.path.as_str()),
                                );
                                let response = response.interact(egui::Sense::click_and_drag());
                                response.dnd_set_drag_payload(FileDrag {
                                    connection: self.identity(),
                                    local: false,
                                    path: entry.path.clone(),
                                    name: entry.name.clone(),
                                    directory,
                                });
                                if response.clicked() {
                                    self.selected_remote = Some(entry.path.clone());
                                }
                                if response.clicked_by(egui::PointerButton::Middle) {
                                    insert_path = Some(entry.path.clone());
                                }
                                if response.double_clicked() && !directory {
                                    if let Some(session) = &self.backend {
                                        open_files.push(crate::editor::OpenFile {
                                            location: crate::editor::Location::Remote {
                                                session: session.clone(),
                                                path: entry.path.clone(),
                                            },
                                            force_text: false,
                                            action: crate::editor::OpenAction::Default,
                                        });
                                    }
                                }
                                if response.double_clicked() && directory {
                                    next_remote = Some(entry.path.clone());
                                }
                                response.context_menu(|ui| {
                                    if ui.button("Open").clicked() {
                                        if directory {
                                            next_remote = Some(entry.path.clone());
                                        } else if let Some(session) = &self.backend {
                                            open_files.push(crate::editor::OpenFile {
                                                location: crate::editor::Location::Remote {
                                                    session: session.clone(),
                                                    path: entry.path.clone(),
                                                },
                                                force_text: false,
                                                action: crate::editor::OpenAction::Default,
                                            });
                                        }
                                        ui.close();
                                    }
                                    if !directory {
                                        for (label, action) in [
                                            (
                                                "Open with…",
                                                crate::editor::OpenAction::ChooseProgram,
                                            ),
                                            (
                                                "Open with default program",
                                                crate::editor::OpenAction::DefaultProgram,
                                            ),
                                        ] {
                                            if ui.button(label).clicked() {
                                                if let Some(session) = &self.backend {
                                                    open_files.push(crate::editor::OpenFile {
                                                        location: crate::editor::Location::Remote {
                                                            session: session.clone(),
                                                            path: entry.path.clone(),
                                                        },
                                                        force_text: false,
                                                        action,
                                                    });
                                                }
                                                ui.close();
                                            }
                                        }
                                        if ui.button("Compare file with…").clicked() {
                                            new_action = Some(FileAction {
                                                operation: "remote_compare".into(),
                                                path: entry.path.clone(),
                                                value: self
                                                    .selected_local
                                                    .clone()
                                                    .unwrap_or_default(),
                                                directory: false,
                                            });
                                            ui.close();
                                        }
                                    }
                                    if !directory && ui.button("Open with text editor").clicked() {
                                        if let Some(session) = &self.backend {
                                            open_files.push(crate::editor::OpenFile {
                                                location: crate::editor::Location::Remote {
                                                    session: session.clone(),
                                                    path: entry.path.clone(),
                                                },
                                                force_text: true,
                                                action: crate::editor::OpenAction::Default,
                                            });
                                        }
                                        ui.close();
                                    }
                                    if ui.button("Copy file path to terminal").clicked() {
                                        insert_path = Some(entry.path.clone());
                                        ui.close();
                                    }
                                    if ui.button("Copy file path").clicked() {
                                        ui.ctx().copy_text(entry.path.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .add_enabled(
                                            !self.local_loaded_path.is_empty(),
                                            egui::Button::new("Copy to this computer…"),
                                        )
                                        .clicked()
                                    {
                                        new_action = Some(FileAction {
                                            operation: "download".into(),
                                            path: entry.path.clone(),
                                            value: PathBuf::from(&self.local_loaded_path)
                                                .join(ad_desktop_model::files::safe_local_name(
                                                    &entry.name,
                                                ))
                                                .to_string_lossy()
                                                .into_owned(),
                                            directory,
                                        });
                                        ui.close();
                                    }
                                    for (label, operation) in [
                                        ("Rename…", "rename"),
                                        ("Properties", "stat"),
                                        ("Permissions…", "chmod"),
                                        ("Delete…", "delete"),
                                    ] {
                                        if ui.button(label).clicked() {
                                            new_action = Some(FileAction {
                                                operation: operation.into(),
                                                path: entry.path.clone(),
                                                value: if operation == "chmod" {
                                                    format!(
                                                        "{:o}",
                                                        entry.permissions.unwrap_or(0o644) & 0o7777
                                                    )
                                                } else {
                                                    entry.path.clone()
                                                },
                                                directory,
                                            });
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        },
                    );
            }
            for (column, local) in [(0, true), (1, false)] {
                if (local && !show_local) || (!local && !show_remote) {
                    continue;
                }
                let drop = columns[column].interact(
                    rects[column],
                    egui::Id::new(("file-drop", local)),
                    egui::Sense::hover(),
                );
                if let Some(payload) = drop.dnd_release_payload::<FileDrag>() {
                    if payload.local != local && (!local || payload.connection == self.identity()) {
                        let destination = if local {
                            PathBuf::from(&self.local_loaded_path)
                                .join(ad_desktop_model::files::safe_local_name(&payload.name))
                                .to_string_lossy()
                                .into_owned()
                        } else {
                            join_remote(&self.remote_loaded_path, &payload.name)
                        };
                        if (local && !self.local_loaded_path.is_empty())
                            || (!local && !self.remote_loaded_path.is_empty())
                        {
                            new_action = Some(FileAction {
                                operation: if local { "download" } else { "upload" }.into(),
                                path: payload.path.clone(),
                                value: destination,
                                directory: payload.directory,
                            });
                        }
                    }
                }
            }
            let hovered = ctx.input(|i| !i.raw.hovered_files.is_empty());
            if hovered {
                self.explorer_hover = ctx.input(|i| i.pointer.latest_pos());
            }
            let dropped = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped.is_empty() {
                let position = ctx
                    .input(|i| i.pointer.latest_pos())
                    .or(self.explorer_hover.take());
                if show_remote
                    && position.is_some_and(|p| rects[1].contains(p))
                    && !self.remote_loaded_path.is_empty()
                {
                    for file in dropped {
                        if let Some(path) = file.path {
                            let result = (|| {
                                let metadata =
                                    std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                                let name = path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .ok_or("Invalid dropped filename")?;
                                let session = self
                                    .backend
                                    .as_ref()
                                    .ok_or("Connect the host before dropping files")?;
                                crate::transfers::enqueue(
                                    session,
                                    true,
                                    path.display().to_string(),
                                    join_remote(&self.remote_loaded_path, name),
                                    metadata.is_dir(),
                                    false,
                                )
                            })();
                            self.status = match result {
                                Ok(()) => {
                                    "Dropped upload queued; existing files will be preserved."
                                        .into()
                                }
                                Err(e) => e,
                            };
                        }
                    }
                }
            }
        }
        ui.allocate_rect(bounds, egui::Sense::hover());
        if let Some(path) = insert_path {
            if matches!(
                self.profile.connection_type,
                ConnectionType::SshTerminal | ConnectionType::SshX11
            ) {
                match ad_desktop_model::files::quote_terminal_path(&path) {
                    Ok(path) => {
                        self.send(Command::Input(format!("{path} ").into_bytes()));
                    }
                    Err(error) => self.status = error.into(),
                }
            }
        }
        self.pending_open.extend(open_files);
        if let Some(path) = next_local {
            self.local_path = path;
            local_go = true;
        }
        if let Some(path) = next_remote {
            self.remote_path = path;
            remote_go = true;
        }
        if local_go {
            self.refresh_local(runtime, &ctx);
        }
        if remote_go {
            self.refresh_remote();
            self.navigation_generation = Some(self.list_generation);
        }
        if let Some(action) = new_action {
            if action.operation == "stat" {
                self.operation(request("stat", action.path));
            } else {
                self.replace_transfers = false;
                self.action = Some(action);
            }
        }
        if let Some(session) = &self.backend {
            crate::transfers::ui(ui, session, runtime);
        }
        for pending in self.pending.values() {
            if let Pending::Operation(progress) = pending {
                ui.horizontal(|ui| {
                    let done = progress.transferred.load(Ordering::Relaxed);
                    let total = progress.total.load(Ordering::Relaxed);
                    ui.label(format!("{done} / {total} bytes"));
                    if ui.button("Cancel transfer").clicked() {
                        progress.cancel();
                    }
                });
            }
        }
    }
    fn action_ui(&mut self, ctx: &egui::Context, runtime: &Connections) {
        let Some(action) = &mut self.action else {
            return;
        };
        let mut open = true;
        let mut execute = false;
        egui::Window::new("Confirm file operation")
            .id(egui::Id::new(("file-action", self.tab_id)))
            .open(&mut open)
            .show(ctx, |ui| {
                let enter = crate::dialog::primary(ui);
                ui.label(format!("{}: {}", action.operation, action.path));
                if action.operation != "delete" && action.operation != "local_delete" {
                    let response = ui.text_edit_singleline(&mut action.value);
                    crate::dialog::focus_on_open(ui, &response);
                } else {
                    ui.label(if action.directory {
                        "Delete this folder and its contents?"
                    } else {
                        "Delete this file?"
                    });
                }
                if action.operation.ends_with("compare") {
                    ui.checkbox(
                        &mut self.compare_remote,
                        "Compare with a file on this server",
                    );
                    ui.label(
                        "Enter the other file's full path, or select it in a file pane first.",
                    );
                }
                if matches!(action.operation.as_str(),"upload"|"download"){ui.checkbox(&mut self.replace_transfers,"Replace existing files after checking their revisions");ui.label("Pause retains verified partial data. Resume requires this same host, account and verified key.");}
                let response = ui.button("Confirm");
                crate::dialog::focus_on_open(ui, &response);
                if response.clicked() || enter {
                    execute = true;
                }
            });
        if execute {
            let action = self.action.take().unwrap();
            if action.operation.ends_with("compare") {
                let source = if action.operation == "local_compare" {
                    crate::editor::Location::Local(PathBuf::from(action.path))
                } else if let Some(session) = &self.backend {
                    crate::editor::Location::Remote {
                        session: session.clone(),
                        path: action.path,
                    }
                } else {
                    self.status = "Connect the server before comparing remote files".into();
                    return;
                };
                let target = if self.compare_remote {
                    if let Some(session) = &self.backend {
                        crate::editor::Location::Remote {
                            session: session.clone(),
                            path: action.value,
                        }
                    } else {
                        self.status = "Connect the server before comparing remote files".into();
                        return;
                    }
                } else {
                    crate::editor::Location::Local(PathBuf::from(action.value))
                };
                self.pending_open.push(crate::editor::OpenFile {
                    location: source,
                    force_text: true,
                    action: crate::editor::OpenAction::Compare(target),
                });
                return;
            }
            if let Some(operation) = action.operation.strip_prefix("local_") {
                if self.local_action_reply.is_some() {
                    self.status = "Wait for the current local operation".into();
                    return;
                }
                let operation = operation.to_owned();
                let path = PathBuf::from(action.path);
                let value = action.value;
                let (sender, receiver) = mpsc::channel(1);
                self.local_action_reply = Some(receiver);
                let ctx = ctx.clone();
                runtime.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        if matches!(operation.as_str(), "properties" | "permissions") {
                            crate::file_workspace::properties(&path, operation == "permissions")
                                .map(|_| None)
                        } else {
                            crate::file_workspace::local_action(&operation, path, value)
                        }
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                    let _ = sender.send(result).await;
                    ctx.request_repaint();
                });
                return;
            }
            let mut req = request(&action.operation, action.path.clone());
            match action.operation.as_str() {
                "rename" => req.destination = Some(action.value),
                "mkdir" => req.path = action.value,
                "chmod" => match u32::from_str_radix(&action.value, 8)
                    .ok()
                    .filter(|mode| *mode <= 0o7777)
                {
                    Some(mode) => req.permissions = Some(mode),
                    None => {
                        self.status = "Enter valid octal permissions, such as 644.".into();
                        return;
                    }
                },
                "delete" => req.recursive = action.directory,
                "upload" => {
                    req.overwrite = self.replace_transfers;
                    req.recursive = action.directory;
                    req.local_path = Some(action.path);
                    req.path = action.value;
                }
                "download" => {
                    req.overwrite = self.replace_transfers;
                    req.recursive = action.directory;
                    req.local_path = Some(action.value);
                }
                _ => {}
            }
            self.operation(req);
        } else if !open {
            self.action = None;
        }
    }
}
impl Drop for SessionTab {
    fn drop(&mut self) {
        self.master_password.zeroize();
        self.deactivate();
        for pending in self.pending.values() {
            if let Pending::Operation(progress) = pending {
                progress.cancel();
            }
        }
    }
}
fn request(operation: &str, path: String) -> RemoteFileRequest {
    RemoteFileRequest {
        operation: operation.into(),
        path,
        local_path: None,
        destination: None,
        expected_revision: None,
        permissions: None,
        overwrite: false,
        recursive: false,
    }
}
fn join_remote(parent: &str, name: &str) -> String {
    let parent = if parent.is_empty() { "." } else { parent };
    format!("{}/{}", parent.trim_end_matches('/'), name)
}
fn remote_parent(path: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        "/".into()
    } else if path == "." {
        "..".into()
    } else {
        match path.rsplit_once('/') {
            Some(("", _)) => "/".into(),
            Some((parent, _)) => parent.into(),
            None => ".".into(),
        }
    }
}
fn retained_desktop_id(profile: &str) -> uuid::Uuid {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        format!("AetherDesk-Rust/desktop/{profile}").as_bytes(),
    );
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_ref()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // Custom deterministic UUID (version 8).
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}
fn files_narrow(width: f32, row_height: f32) -> bool {
    width < 700.0_f32.max(row_height * 48.0)
}
fn file_pane_rects(bounds: egui::Rect, narrow: bool, gap: f32) -> [egui::Rect; 2] {
    if narrow {
        return [bounds; 2];
    }
    let width = ((bounds.width() - gap).max(0.0)) * 0.5;
    [
        egui::Rect::from_min_size(bounds.min, egui::vec2(width, bounds.height())),
        egui::Rect::from_min_size(
            egui::pos2(bounds.right() - width, bounds.top()),
            egui::vec2(width, bounds.height()),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_sync_switch_gates_both_directions_and_invalidates_pending_updates() {
        for kind in [ConnectionType::SshTerminal, ConnectionType::SshX11] {
            let mut profile = crate::profile_store::new_profile();
            profile.connection_type = kind;
            let mut tab = SessionTab::new(profile);
            tab.remote_loaded_path = "/home/AR/files".into();
            tab.set_directory_sync(false);
            tab.terminal
                .process(b"\x1b]7;file://fic/home/AR/terminal\x07");
            assert!(tab.pending_shell_directory().is_none());
            tab.navigation_generation = Some(4);
            assert_eq!(tab.pending_terminal_navigation(4).unwrap(), None);
            tab.set_directory_sync(true);
            assert!(
                tab.pending_shell_directory().is_none(),
                "old disabled OSC updates must not replay"
            );
            tab.terminal
                .process(b"\x1b]7;file://fic/home/AR/terminal\x07");
            assert_eq!(
                tab.pending_shell_directory().as_deref(),
                Some("/home/AR/terminal")
            );
            tab.navigation_generation = Some(5);
            assert_eq!(
                tab.pending_terminal_navigation(5).unwrap().as_deref(),
                Some("'/home/AR/files'")
            );
            tab.list_generation = 6;
            tab.sync_generation = Some(6);
            tab.navigation_generation = Some(6);
            tab.remote_path = "/home/AR/pending".into();
            tab.set_directory_sync(false);
            assert_eq!(tab.list_generation, 7);
            assert_eq!(tab.remote_path, tab.remote_loaded_path);
            tab.set_directory_sync(true);
            assert!(tab.pending_terminal_navigation(6).unwrap().is_none());
        }
    }
    #[test]
    fn files_reflow_into_one_pane_without_overlap_or_losing_navigation() {
        let runtime = Connections::new().unwrap();
        let directory = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(directory.path(), || {
            let mut tab = SessionTab::diagnostic_files(directory.path());
            for font in [13.0, 20.0] {
                let ctx = egui::Context::default();
                ctx.style_mut(|style| {
                    for key in [egui::TextStyle::Body, egui::TextStyle::Button] {
                        style
                            .text_styles
                            .insert(key, egui::FontId::proportional(font));
                    }
                });
                for width in [360.0, 480.0, 700.0, 1200.0, 480.0] {
                    for local in [false, true] {
                        tab.files_local_pane = local;
                        let mut output = None;
                        let mut narrow = false;
                        for _ in 0..4 {
                            output = Some(ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(egui::Rect::from_min_size(
                                        egui::Pos2::ZERO,
                                        egui::vec2(width, 420.0),
                                    )),
                                    ..Default::default()
                                },
                                |ctx| {
                                    egui::CentralPanel::default().show(ctx, |ui| {
                                        narrow = files_narrow(
                                            ui.available_width(),
                                            ui.text_style_height(&egui::TextStyle::Body),
                                        );
                                        tab.files_ui(ui, &runtime);
                                    });
                                },
                            ));
                        }
                        let mut visible = Vec::new();
                        for shape in output.unwrap().shapes {
                            if let egui::epaint::Shape::Text(text) = shape.shape {
                                if ["local-fixture", "server-fixture"]
                                    .contains(&text.galley.job.text.as_str())
                                {
                                    let rect =
                                        egui::Rect::from_min_size(text.pos, text.galley.size());
                                    assert!(shape.clip_rect.expand(1.0).contains_rect(rect),"File entry escaped its pane at width {width}, font {font}: {rect:?} / {:?}",shape.clip_rect);
                                    visible.push(text.galley.job.text.clone());
                                }
                            }
                        }
                        if narrow {
                            assert_eq!(
                                visible,
                                vec![if local {
                                    "local-fixture"
                                } else {
                                    "server-fixture"
                                }]
                            );
                        } else {
                            assert_eq!(visible, vec!["local-fixture", "server-fixture"]);
                        }
                        assert_eq!(tab.remote_loaded_path, "/fixture");
                        assert_eq!(
                            tab.local_loaded_path,
                            directory.path().display().to_string()
                        );
                        assert!(tab.connected);
                        assert!(tab.pending.is_empty());
                    }
                }
            }
        });
    }
    #[test]
    fn files_popup_does_not_shrink_or_disconnect_the_desktop_or_terminal() {
        let runtime = Connections::new().unwrap();
        for kind in [
            ConnectionType::RemoteDesktop,
            ConnectionType::SshTerminal,
            ConnectionType::SshX11,
        ] {
            let ctx = egui::Context::default();
            let mut profile = crate::profile_store::new_profile();
            profile.connection_type = kind;
            let mut tab = SessionTab::new(profile);
            tab.connected = true;
            tab.credentials_open = false;
            let render = |tab: &mut SessionTab| {
                let mut remaining = egui::Rect::NOTHING;
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            tab.ui(ui, &runtime);
                            remaining = ui.min_rect();
                        });
                    },
                );
                remaining
            };
            // Stabilize egui's first layout pass before comparing.
            render(&mut tab);
            let before = render(&mut tab);
            tab.files_visible = true;
            let after = render(&mut tab);
            assert_eq!(
                before, after,
                "Files must leave the main session geometry unchanged"
            );
            let status = tab.status.clone();
            tab.close_files();
            assert!(tab.connected);
            assert_eq!(tab.status, status);
            assert!(!tab.files_visible);
        }
    }
    #[cfg(windows)]
    #[test]
    fn desktop_password_opt_in_survives_store_reload_and_can_be_revoked() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = crate::vault::delete(&self.0);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(folder.path(), || {
            let mut profile = crate::profile_store::new_profile();
            profile.host = "fixture.invalid".into();
            profile.connection_type = ConnectionType::RemoteDesktop;
            profile.username = "old-user".into();
            profile.remember_username = true;
            profile.remember_password = false;
            let _cleanup = Cleanup(format!("ssh-{}", profile.id));
            let mut profiles = vec![profile.clone()];
            let mut tab = SessionTab::new(profile);
            let runtime = Connections::new().unwrap();
            let ctx = egui::Context::default();
            // Exercise the same successful-authentication handler without opening
            // any network connection. Only this fixture UUID enters the vault.
            tab.profile.remember_password = true;
            tab.remember_changed = true;
            tab.credentials.username = "new-user".into();
            tab.credentials.secret = "desktop-fixture-secret".into();
            tab.authenticated_credentials(&runtime, &ctx);
            tab.credentials.secret.zeroize();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while tab.credential_reply.is_some() {
                tab.poll(&runtime, &ctx);
                assert!(std::time::Instant::now() < deadline, "{}", tab.status);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let update = tab
                .take_profile_update()
                .expect("successful vault write commits consent");
            assert!(crate::profile_store::credential_update(
                &mut profiles,
                update
            ));
            crate::profile_store::save(&profiles).unwrap();
            let saved_bytes = std::fs::read(folder.path().join("sessions.json")).unwrap();
            assert!(!String::from_utf8(saved_bytes)
                .unwrap()
                .contains("desktop-fixture-secret"));
            let reloaded = crate::profile_store::load().unwrap().remove(0);
            assert!(reloaded.remember_password && reloaded.remember_username);
            assert_eq!(reloaded.username, "new-user");
            let mut reopened = SessionTab::new(reloaded);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while reopened.credentials.secret != "desktop-fixture-secret" {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| reopened.ui(ui, &runtime));
                });
                reopened.poll(&runtime, &ctx);
                assert!(std::time::Instant::now() < deadline, "{}", reopened.status);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(!reopened.connected && reopened.connection.is_none());
            reopened.profile.remember_password = false;
            reopened.remember_changed = true;
            reopened.authenticated_credentials(&runtime, &ctx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while reopened.credential_reply.is_some() {
                reopened.poll(&runtime, &ctx);
                assert!(std::time::Instant::now() < deadline, "{}", reopened.status);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let update = reopened.take_profile_update().unwrap();
            assert!(crate::profile_store::credential_update(
                &mut profiles,
                update
            ));
            crate::profile_store::save(&profiles).unwrap();
            assert!(!crate::profile_store::load().unwrap()[0].remember_password);
            assert!(crate::vault::read(&_cleanup.0).unwrap().is_none());
        });
    }
    #[cfg(windows)]
    #[test]
    fn saved_password_loads_on_open_and_reconnect_without_manual_unlock() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = crate::vault::delete(&self.0);
            }
        }
        let folder = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(folder.path(), || {
            let mut profile = crate::profile_store::new_profile();
            profile.host = "fixture.invalid".into();
            profile.username = "fixture-user".into();
            profile.remember_username = true;
            profile.remember_password = true;
            let _cleanup = Cleanup(format!("ssh-{}", profile.id));
            crate::credentials::remember(&profile, &profile.username, "fixture-password", "")
                .unwrap();
            let mut tab = SessionTab::new(profile);
            let runtime = Connections::new().unwrap();
            let ctx = egui::Context::default();
            for attempt in 0..2 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while tab.credentials.secret != "fixture-password" {
                    let _ = ctx.run(egui::RawInput::default(), |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            tab.ui(ui, &runtime);
                        });
                    });
                    tab.poll(&runtime, &ctx);
                    assert!(
                        std::time::Instant::now() < deadline,
                        "credential load timed out: {}",
                        tab.status
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert!(tab.credential_loaded);
                assert!(!tab.connected); // Loading a secret never starts a network connection.
                if attempt == 0 {
                    tab.disconnect();
                    tab.open_credentials();
                }
            }
        });
    }
    #[test]
    fn retained_desktop_identity_survives_reopening_and_differs_between_profiles() {
        assert_eq!(
            retained_desktop_id("saved-one"),
            retained_desktop_id("saved-one")
        );
        assert_ne!(
            retained_desktop_id("saved-one"),
            retained_desktop_id("saved-two")
        );
        let profile = SavedSession::from_json(
            &serde_json::json!({"id":"saved-one","name":"one","host":"server"}),
        )
        .unwrap();
        let mut duplicate = SessionTab::new(profile);
        let primary = duplicate.desktop_id;
        duplicate.make_independent_desktop();
        assert_ne!(primary, duplicate.desktop_id);
        let restored = SessionTab::recover(
            duplicate.profile.clone(),
            duplicate.desktop_id,
            "alice".into(),
        );
        assert_eq!(restored.desktop_id, duplicate.desktop_id);
        assert_eq!(restored.credentials.username, "alice");
    }
    #[test]
    fn remote_navigation_does_not_use_windows_path_rules() {
        assert_eq!(join_remote("/", "a b"), "/a b");
        assert_eq!(join_remote("", "a"), "./a");
        assert_eq!(remote_parent("/home/me/"), "/home");
        assert_eq!(remote_parent("/home"), "/");
        assert_eq!(remote_parent("/"), "/");
    }
}
