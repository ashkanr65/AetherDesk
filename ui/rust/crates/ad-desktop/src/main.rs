#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod ai_api;
mod ai_models;
mod ai_tools;
mod app_layout;
mod app_tools;
mod assistant;
mod assistant_history;
mod comparison;
mod connection;
mod crash_diagnostics;
mod credentials;
mod desktop_recovery;
mod dialog;
mod editor;
mod editor_language;
mod editor_tools;
mod editor_transport;
mod file_picker;
mod file_workspace;
mod graphics;
mod log_panel;
mod logging;
mod migration;
mod navigation;
mod notifications;
mod preferences;
mod process_health;
mod process_job;
mod profile_store;
#[cfg(all(test, windows))]
mod r12_acceptance;
mod r12_ui;
mod remote;
mod remote_applications;
mod remote_clipboard;
mod remote_cursor;
mod remote_frames;
mod remote_geometry;
mod remote_input;
mod remote_keyboard;
mod remote_keypad;
mod remote_mouse;
mod remote_toolbar;
mod remote_view;
mod server_clock;
mod session;
mod session_form;
mod storage;
mod terminal;
mod terminal_navigation;
mod terminal_settings;
mod theme;
mod theme_palette;
mod transfers;
mod ui_icons;
mod update_apply;
mod update_manifest;
mod updates;
mod user_tools;
mod vault;
mod window_geometry;
mod working_copies;
mod x11;
use ad_desktop_model::{
    profiles::SavedSession,
    state::{Sidebar, WorkspaceState},
};
use eframe::egui;
use std::path::PathBuf;
use zeroize::Zeroize;

struct RustDesktop {
    workspace: WorkspaceState<session::SessionTab>,
    connections: Result<connection::Connections, String>,
    profiles: Vec<SavedSession>,
    filter: String,
    notice: String,
    dark: bool,
    new_profile: Option<session_form::Draft>,
    closing: Option<std::time::Instant>,
    editors: editor::Editors,
    assistant: assistant::Assistant,
    tools: user_tools::Panel,
    profiles_valid: bool,
    profile_io: Option<bool>,
    profile_path: String,
    app_requests: tokio::sync::mpsc::Receiver<app_tools::Request>,
    logs: log_panel::Panel,
    settings_valid: bool,
    show_desktop_recovery: bool,
    theme: theme::Theme,
    show_appearance: bool,
    preferences: preferences::Panel,
    confirm_quit_open: bool,
    quit_confirmed: bool,
    show_logs: bool,
    show_help: bool,
    show_about: bool,
    show_migration: bool,
    migrate_credentials: bool,
    migration_master: String,
    tab_strip: navigation::Strip,
    notifications: notifications::Center,
    last_notice: String,
    updates: updates::Updates,
    update_health: Option<PathBuf>,
}
impl Default for RustDesktop {
    fn default() -> Self {
        let loaded = profile_store::load();
        let profiles_valid = loaded.is_ok();
        let settings = storage::read_json("settings.json");
        let settings_valid = settings.is_ok();
        let settings = settings.unwrap_or_default().unwrap_or_default();
        let mut workspace = WorkspaceState::default();
        workspace.sidebar_width = settings["sidebarWidth"]
            .as_f64()
            .filter(|w| w.is_finite())
            .unwrap_or(214.0)
            .clamp(190.0, 900.0) as f32;
        workspace.sidebar_visible = settings["sidebarVisible"].as_bool().unwrap_or(true);
        let notice = loaded.as_ref().err().cloned().unwrap_or_default();
        let dark = settings["dark"].as_bool().unwrap_or(true);
        let preferences = preferences::Panel::load(dark);
        let mut app = Self {
            workspace,
            connections: connection::Connections::new(),
            profiles: loaded.unwrap_or_default(),
            filter: String::new(),
            notice,
            dark,
            new_profile: None,
            closing: None,
            editors: editor::Editors::default(),
            assistant: assistant::Assistant::default(),
            tools: user_tools::Panel::default(),
            profiles_valid,
            profile_io: None,
            profile_path: String::new(),
            app_requests: app_tools::initialize(),
            logs: log_panel::Panel::default(),
            settings_valid,
            show_desktop_recovery: false,
            theme: theme::Theme::default(),
            show_appearance: false,
            preferences,
            confirm_quit_open: false,
            quit_confirmed: false,
            show_logs: false,
            show_help: false,
            show_about: false,
            show_migration: false,
            migrate_credentials: false,
            migration_master: String::new(),
            tab_strip: navigation::Strip::default(),
            notifications: notifications::Center::default(),
            last_notice: String::new(),
            updates: updates::Updates::default(),
            update_health: if std::env::args().nth(1).as_deref() == Some("--update-health") {
                std::env::args_os().nth(2).map(PathBuf::from)
            } else {
                None
            },
        };
        if app.preferences.settings.restore_tabs {
            if let Some(ids) = settings["openSessions"].as_array() {
                for id in ids.iter().take(32).filter_map(|v| v.as_str()) {
                    if let Some(profile) = app.profiles.iter().find(|p| p.id == id).cloned() {
                        app.open_profile(profile);
                        if let Some(tab) = app.workspace.tabs.last_mut() {
                            tab.restore_disconnected();
                        }
                    }
                }
            }
        }
        app
    }
}
fn palette(ctx: &egui::Context, dark: bool) {
    theme::apply(ctx, dark);
}
impl RustDesktop {
    fn workspace_sidebar(&mut self, ctx: &egui::Context) {
        let maximum = app_layout::sidebar_maximum(ctx.available_rect().width());
        if self.workspace.sidebar_visible && maximum >= 180.0 {
            let panel = egui::Id::new("workspace");
            let resize = ctx.read_response(panel.with("__resize"));
            let resizing = resize
                .as_ref()
                .is_some_and(|r| r.is_pointer_button_down_on() || r.drag_stopped());
            if !resizing {
                // A temporary small monitor/window must not overwrite the user's
                // preferred sidebar width, or leave it stuck at the clamped size.
                if let Some(mut state) = egui::containers::panel::PanelState::load(ctx, panel) {
                    state.rect.max.x =
                        state.rect.min.x + self.workspace.sidebar_width.clamp(180.0, maximum);
                    ctx.data_mut(|d| d.insert_persisted(panel, state));
                }
            }
            let width = egui::SidePanel::left("workspace")
                .resizable(true)
                .default_width(self.workspace.sidebar_width.clamp(180.0, maximum))
                .width_range(180.0..=maximum)
                .show(ctx, |ui| self.sidebar_contents(ui))
                .response
                .rect
                .width();
            if ctx
                .read_response(panel.with("__resize"))
                .is_some_and(|r| r.drag_stopped())
            {
                self.workspace.sidebar_width = width;
                self.persist_settings();
            }
            if ctx
                .read_response(panel.with("__resize"))
                .is_some_and(|r| r.double_clicked())
            {
                if let Some(mut state) = egui::containers::panel::PanelState::load(ctx, panel) {
                    self.workspace.sidebar_width = 214.0_f32.min(maximum);
                    state.rect.max.x = state.rect.min.x + self.workspace.sidebar_width;
                    self.persist_settings();
                    ctx.data_mut(|d| d.insert_persisted(panel, state));
                    ctx.request_repaint();
                }
            }
        } else if self.workspace.sidebar_visible {
            // At phone-like widths, show the chosen sidebar as a bounded drawer
            // rather than crush the terminal/desktop and chat into each other.
            let mut open = true;
            let screen = ctx.content_rect();
            egui::Window::new(self.workspace.sidebar.label())
                .id(egui::Id::new("compact-sidebar"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_pos(screen.min + egui::vec2(48.0, 52.0))
                .fixed_size(egui::vec2(
                    self.workspace
                        .sidebar_width
                        .max(240.0)
                        .min((screen.width() - 70.0).max(80.0)),
                    (screen.height() - 130.0).max(80.0),
                ))
                .show(ctx, |ui| self.sidebar_contents(ui));
            if !open {
                self.workspace.sidebar_visible = false;
            }
        }
    }
    fn sidebar_contents(&mut self, ui: &mut egui::Ui) {
        self.theme.sidebar(ui, self.dark);
        ui.add_space(10.0);
        navigation::sidebar_body(ui, self.workspace.sidebar.label(), |ui| {
            match self.workspace.sidebar {
                Sidebar::Sessions | Sidebar::Bookmarks => {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text("Search connections")
                            .margin(egui::Margin::symmetric(9, 8))
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(if self.workspace.sidebar == Sidebar::Bookmarks {
                                "BOOKMARKS"
                            } else {
                                "SAVED SESSIONS"
                            })
                            .size(10.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("+").clicked() {
                                self.new_profile = Some(profile_store::new_profile().into());
                            }
                        });
                    });
                    ui.add_space(8.0);
                    let query = self.filter.to_lowercase();
                    let mut action = None;
                    let mut open_profile = None;
                    for (index, profile) in self
                        .profiles
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| !p.is_deleted())
                    {
                        if self.workspace.sidebar == Sidebar::Bookmarks && !profile.favorite {
                            continue;
                        }
                        if !format!("{} {} {}", profile.name, profile.host, profile.username)
                            .to_lowercase()
                            .contains(&query)
                        {
                            continue;
                        }
                        let selected = self
                            .workspace
                            .selected
                            .and_then(|i| self.workspace.tabs.get(i))
                            .is_some_and(|t| t.profile.id == profile.id);
                        let c = theme::colors(self.dark);
                        let response = crate::ui_icons::session_entry(
                            ui,
                            &format!(
                                "{}{}",
                                if profile.favorite { "★ " } else { "" },
                                profile.name
                            ),
                            &format!(
                                "{}@{} · {}",
                                profile.username,
                                profile.host,
                                profile.connection_type.label()
                            ),
                            if selected { c.selection_text } else { c.text },
                            if selected {
                                c.selection
                            } else {
                                c.selection.gamma_multiply(0.65)
                            },
                            egui::Stroke::new(if selected { 1.5_f32 } else { 0.6_f32 }, c.border),
                        );
                        if response.clicked() && !profile.is_deleted() {
                            open_profile = Some(profile.clone());
                        }
                        response.context_menu(|ui| {
                            for (title, kind) in if profile.is_deleted() {
                                vec![("Restore", "restore"), ("Delete permanently", "purge")]
                            } else {
                                vec![
                                    ("Edit", "edit"),
                                    ("Duplicate", "duplicate"),
                                    (
                                        if profile.favorite {
                                            "Remove bookmark"
                                        } else {
                                            "Bookmark"
                                        },
                                        "favorite",
                                    ),
                                    ("Delete session", "trash"),
                                ]
                            } {
                                if ui.button(title).clicked() {
                                    action = Some((index, kind));
                                    ui.close();
                                }
                            }
                        });
                    }
                    if let Some(profile) = open_profile {
                        self.open_profile(profile);
                    }
                    if let Some((index, kind)) = action {
                        match kind {
                            "edit" => self.new_profile = Some(self.profiles[index].clone().into()),
                            "duplicate" => {
                                let mut p = self.profiles[index].clone();
                                p.id = uuid::Uuid::new_v4().to_string();
                                p.name.push_str(" (copy)");
                                p.remember_password = false;
                                p.master_password_protected = false;
                                self.new_profile = Some(p.into());
                            }
                            "favorite" => {
                                self.profiles[index].favorite = !self.profiles[index].favorite
                            }
                            "trash" => {
                                self.profiles[index].deleted_at =
                                    Some(chrono::Utc::now().to_rfc3339())
                            }
                            "restore" => self.profiles[index].deleted_at = None,
                            "purge" => {
                                match crate::vault::delete(&format!(
                                    "ssh-{}",
                                    self.profiles[index].id
                                )) {
                                    Ok(()) => {
                                        self.profiles.remove(index);
                                    }
                                    Err(error) => self.report_error(error),
                                }
                            }
                            _ => {}
                        }
                        if !matches!(kind, "edit" | "duplicate") {
                            self.persist_profiles();
                        }
                    }
                }
                Sidebar::Tools => {
                    let workspace = self
                        .workspace
                        .selected
                        .and_then(|i| self.workspace.tabs.get(i))
                        .and_then(|t| t.ai_workspace())
                        .unwrap_or_else(|| {
                            crate::ai_tools::Workspace::Local(
                                std::env::var("USERPROFILE")
                                    .unwrap_or_else(|_| ".".into())
                                    .into(),
                            )
                        });
                    if let Ok(runtime) = &self.connections {
                        self.tools.ui(ui, runtime, workspace);
                    }
                }
                Sidebar::Assistant => {
                    let tab = self
                        .workspace
                        .selected
                        .and_then(|i| self.workspace.tabs.get(i));
                    let connection = tab.and_then(|t| t.ai_workspace());
                    let capture = tab.and_then(|t| t.capture());
                    if let Ok(runtime) = &self.connections {
                        self.assistant.ui(ui, runtime, connection, capture);
                    }
                }
                Sidebar::Connections => {
                    let mut selected = None;
                    let mut disconnect = None;
                    for (index, tab) in self
                        .workspace
                        .tabs
                        .iter()
                        .enumerate()
                        .filter(|(_, t)| t.is_active())
                    {
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if crate::ui_icons::button(
                                        ui,
                                        crate::ui_icons::Icon::Disconnect,
                                        "Disconnect",
                                        false,
                                    )
                                    .clicked()
                                    {
                                        disconnect = Some(index);
                                    }
                                    if crate::ui_icons::connection_entry(
                                        ui,
                                        &tab.profile.name,
                                        tab.state_label(),
                                        self.workspace.selected == Some(index),
                                    )
                                    .on_hover_text(format!(
                                        "{} · {}",
                                        tab.profile.name,
                                        tab.state_label()
                                    ))
                                    .clicked()
                                    {
                                        selected = Some(index);
                                    }
                                },
                            );
                        });
                    }
                    if let Some(index) = selected {
                        self.select_tab(index);
                    }
                    if let Some(index) = disconnect {
                        self.workspace.tabs[index].disconnect();
                    }
                    if !self.workspace.tabs.iter().any(|t| t.is_active()) {
                        ui.label("No active connections");
                    }
                }
                Sidebar::Logs => {
                    self.show_logs = true;
                    ui.label("Log files are open in a popup.");
                }
                _ => {
                    ui.label("This sidebar's functional port is pending. See RUST-FEATURE-PARITY.md for the tracked behavior.");
                }
            }
        });
    }
    fn report_error(&mut self, error: String) {
        self.last_notice = error.clone();
        self.notice = error.clone();
        self.notifications.push(notifications::Notice::new(
            notifications::Kind::Error,
            error,
        ));
    }
    fn select_tab(&mut self, index: usize) {
        if index >= self.workspace.tabs.len() || self.workspace.selected == Some(index) {
            return;
        }
        if let Some(old) = self
            .workspace
            .selected
            .and_then(|i| self.workspace.tabs.get_mut(i))
        {
            old.deactivate();
        }
        self.workspace.selected = Some(index);
    }
    fn open_profile(&mut self, profile: SavedSession) {
        let duplicate = self
            .workspace
            .tabs
            .iter()
            .any(|t| t.profile.id == profile.id);
        let mut tab = session::SessionTab::new(profile);
        if duplicate {
            tab.make_independent_desktop();
        }
        if let Some(previous) = self
            .workspace
            .selected
            .and_then(|i| self.workspace.tabs.get_mut(i))
        {
            previous.deactivate();
        }
        self.workspace.open_new(tab);
    }
    fn finish_connection_form(&mut self, action: Option<session_form::Action>) {
        match action {
            Some(session_form::Action::Save | session_form::Action::Open) => {
                if let Some(draft) = &self.new_profile {
                    let profile = draft.profile.clone();
                    let mut next = self.profiles.clone();
                    next.retain(|p| p.id != profile.id);
                    next.push(profile.clone());
                    let result = if !profile.remember_password
                        && self
                            .profiles
                            .iter()
                            .any(|p| p.id == profile.id && p.remember_password)
                    {
                        vault::delete(&format!("ssh-{}", profile.id))
                    } else {
                        Ok(())
                    }
                    .and_then(|_| profile_store::save(&next));
                    match result {
                        Ok(()) => {
                            self.profiles = next;
                            if let Some(draft) = self.new_profile.take() {
                                if matches!(action, Some(session_form::Action::Open)) {
                                    self.open_profile(draft.profile.clone());
                                    if let Some(tab) = self.workspace.tabs.last_mut() {
                                        tab.prefill_credentials(
                                            (*draft.secret).clone(),
                                            (*draft.master).clone(),
                                        );
                                    }
                                }
                            }
                            self.notice = "Connection saved".into();
                        }
                        Err(error) => self.report_error(error),
                    }
                }
            }
            Some(session_form::Action::Cancel) => self.new_profile = None,
            None => {}
        }
    }
    fn persist_settings(&mut self) {
        if self.settings_valid {
            let mut settings = storage::read_json("settings.json")
                .ok()
                .flatten()
                .filter(|v| v.is_object())
                .unwrap_or_else(|| serde_json::json!({}));
            settings["dark"] = self.dark.into();
            settings["sidebarWidth"] = self.workspace.sidebar_width.into();
            settings["sidebarVisible"] = self.workspace.sidebar_visible.into();
            settings["openSessions"] = serde_json::json!(self
                .workspace
                .tabs
                .iter()
                .map(|t| t.profile.id.clone())
                .collect::<Vec<_>>());
            if let Err(error) = storage::write_json("settings.json", &settings) {
                self.report_error(error);
            }
        }
    }
    fn app_action(
        &mut self,
        name: &str,
        args: &serde_json::Value,
        ctx: &egui::Context,
    ) -> Result<app_tools::ResultValue, String> {
        use app_tools::ResultValue;
        use serde_json::json;
        if name == "app_list_connections" {
            return Ok(ResultValue::Json(json!(self.workspace.tabs.iter().map(|t|json!({"id":t.identity(),"name":t.profile.name,"connected":t.connected,"type":t.profile.connection_type.name()})).collect::<Vec<_>>())));
        }
        if name == "app_show_panel" {
            let panel = Sidebar::ALL
                .into_iter()
                .find(|s| Some(s.label()) == args["panel"].as_str())
                .ok_or("Unknown panel")?;
            if panel == Sidebar::Logs {
                self.show_logs = true;
            } else {
                self.workspace.select_sidebar(panel);
            }
            return Ok(ResultValue::Json(json!({"shown":panel.label()})));
        }
        let id = args["connection_id"]
            .as_str()
            .ok_or("Connection identity is required")?;
        let path = args["path"].as_str().unwrap_or("");
        if path.len() > 16384 || path.contains('\0') {
            return Err("Invalid path".into());
        }
        if name == "app_open_file" && id.is_empty() {
            let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
            let runtime = self.connections.as_ref().map_err(|e| e.clone())?;
            self.editors.open(
                editor::OpenFile {
                    location: editor::Location::Local(path),
                    force_text: false,
                    action: crate::editor::OpenAction::Default,
                },
                runtime,
                ctx,
            );
            return Ok(ResultValue::Json(json!({"opening_requested":true})));
        }
        let index = self
            .workspace
            .tabs
            .iter()
            .position(|t| t.identity() == id)
            .ok_or("That connection no longer exists; list connections again")?;
        if name == "app_select_connection" {
            if let Some(old) = self
                .workspace
                .selected
                .and_then(|i| self.workspace.tabs.get_mut(i))
            {
                old.deactivate();
            }
            self.workspace.selected = Some(index);
            return Ok(ResultValue::Json(json!({"selected":id})));
        }
        let tab = &mut self.workspace.tabs[index];
        match name {
            "app_capture_screen" => tab
                .capture()
                .map(ResultValue::Screen)
                .ok_or_else(|| "No remote desktop frame is available".into()),
            "app_terminal_text" => Ok(ResultValue::Json(json!({"text":tab.terminal_text()?}))),
            "app_open_file" => {
                tab.open_remote_file(path.into())?;
                Ok(ResultValue::Json(json!({"opening_requested":true})))
            }
            "app_open_folder" => {
                tab.open_remote_folder(path.into())?;
                Ok(ResultValue::Json(json!({"navigation_requested":true})))
            }
            _ => Err("Unknown application action".into()),
        }
    }
    fn persist_profiles(&mut self) {
        if !self.profiles_valid {
            self.notice =
                "The damaged native session file was preserved; restore it before saving profiles."
                    .into();
            return;
        }
        if let Err(error) = profile_store::save(&self.profiles) {
            self.report_error(error);
        }
    }
    fn import_profiles(&mut self) {
        // Explicit read-only import. Never mutate the production application's profile.
        let result = std::env::var_os("APPDATA")
            .ok_or("APPDATA is unavailable".to_owned())
            .and_then(|root| {
                profile_store::import(&PathBuf::from(root).join("AetherDesk/sessions.json"))
            });
        match result {
            Ok(profiles) => {
                self.profiles.extend(profiles);
                self.notice = "Profiles imported. Existing profiles and credentials are unchanged."
                    .to_owned();
                self.persist_profiles();
            }
            Err(error) => self.report_error(error),
        }
    }
}
impl eframe::App for RustDesktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        process_health::pulse(1);
        app_layout::update(ctx, "main");
        self.theme.initialize(ctx);
        let dark = self.preferences.settings.dark(ctx, self.dark);
        if dark != self.dark {
            self.dark = dark;
            palette(ctx, self.dark);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        // Navigation shortcuts are consumed below, after checking whether a
        // remote desktop owns the keyboard. Do not consume Ctrl+Shift+K here.
        process_health::pulse(2);
        self.assistant.poll(ctx);
        let had_update = self.updates.available();
        self.updates.poll(ctx);
        if !had_update && self.updates.available() {
            self.notifications.push(notifications::Notice::new(
                notifications::Kind::Information,
                "An AetherDesk update is available. Open the gear menu → Updates to review it.",
            ));
        }
        while let Ok(request) = self.app_requests.try_recv() {
            // A cancelled approval/turn must never cause a delayed UI action.
            if request.reply.is_closed() {
                continue;
            }
            let result = if self.closing.is_some() {
                Err("Application is closing".into())
            } else {
                self.app_action(&request.name, &request.args, ctx)
            };
            let _ = request.reply.send(result);
        }
        if let Ok(runtime) = &self.connections {
            for tab in &mut self.workspace.tabs {
                for file in std::mem::take(&mut tab.pending_open) {
                    self.editors.open(file, runtime, ctx);
                }
            }
            self.editors.ui(ctx, runtime);
            let sessions: Vec<_> = self
                .workspace
                .tabs
                .iter()
                .filter_map(|t| t.ssh_session())
                .collect();
            self.editors.recovery_ui(ctx, runtime, &sessions);
        }
        if self.confirm_quit_open {
            let mut open = true;
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Quit AetherDesk?").open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
                ui.label("Active connections will be disconnected. Saved sessions and retained desktops remain available.");
                ui.horizontal(|ui| { confirm = ui.button("Quit").clicked(); cancel = ui.button("Cancel").clicked(); });
            });
            if confirm {
                self.quit_confirmed = true;
                self.confirm_quit_open = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if cancel || !open {
                self.confirm_quit_open = false;
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.editors.can_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.request_repaint();
        } else if ctx.input(|i| i.viewport().close_requested())
            && self.closing.is_none()
            && self.preferences.settings.confirm_quit
            && !self.quit_confirmed
            && self.workspace.tabs.iter().any(|t| t.is_active())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit_open = true;
            ctx.request_repaint();
        } else if ctx.input(|i| i.viewport().close_requested()) && self.closing.is_none() {
            self.persist_settings();
            self.closing = Some(std::time::Instant::now());
            self.assistant.stop();
            self.tools.stop();
            transfers::pause_all();
            if let Ok(runtime) = &self.connections {
                runtime.begin_shutdown();
            }
        }
        if let Some(start) = self.closing {
            let pending = self
                .connections
                .as_ref()
                .is_ok_and(|runtime| !runtime.is_idle());
            if pending && start.elapsed() < std::time::Duration::from_secs(75) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
                process_health::pulse(3);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Closing connections…");
                    ui.label("Finishing file operations and releasing SSH sessions.");
                });
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        // Remote desktop owns host shortcuts while its surface has key focus.
        let remote_keyboard = self
            .workspace
            .tabs
            .iter()
            .any(|tab| tab.captures_remote_keyboard());
        ctx.options_mut(|options| options.zoom_with_keyboard = !remote_keyboard);
        let shortcuts = if remote_keyboard {
            vec![]
        } else {
            navigation::take_shortcuts(ctx)
        };
        // Run on the main viewport only. Editor popups own their own shortcuts.
        for action in shortcuts {
            match action {
                navigation::Action::NewConnection => {
                    self.new_profile = Some(profile_store::new_profile().into())
                }
                navigation::Action::ToggleSidebar => {
                    self.workspace.sidebar_visible = !self.workspace.sidebar_visible
                }
                navigation::Action::Relative(delta) => {
                    if let Some(index) = self.workspace.relative_tab(delta) {
                        self.select_tab(index);
                    }
                }
            }
        }
        let mut credential_preferences_changed = false;
        let mut show_connected = false;
        if let Ok(runtime) = &self.connections {
            for (index, tab) in self.workspace.tabs.iter_mut().enumerate() {
                let was_connected = tab.connected;
                tab.poll(runtime, ctx);
                show_connected |=
                    !was_connected && tab.connected && self.workspace.selected == Some(index);
                if let Some(update) = tab.take_profile_update() {
                    credential_preferences_changed |=
                        profile_store::credential_update(&mut self.profiles, update);
                }
                for notice in tab.take_notices() {
                    self.notifications.push(notice);
                }
            }
            let sessions: Vec<_> = self
                .workspace
                .tabs
                .iter()
                .filter_map(|t| t.ssh_session())
                .collect();
            transfers::poll(runtime, &sessions, ctx);
            transfers::window(ctx, &sessions, runtime);
        }
        if show_connected {
            self.workspace.select_sidebar(Sidebar::Connections);
        }
        if credential_preferences_changed {
            self.persist_profiles();
        }
        egui::TopBottomPanel::top("toolbar")
            .exact_height(47.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ctx, |ui| {
                self.theme.toolbar(ui, self.dark);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.menu_button("☰", |ui| {
                        if ui.button("Show / hide sidebar").clicked() {
                            self.workspace.sidebar_visible = !self.workspace.sidebar_visible;
                            ui.close();
                        }
                        if ui.button("Import saved sessions (read only)").clicked() {
                            self.import_profiles();
                            ui.close();
                        }
                        if ui.button("Import previous settings…").clicked() {
                            self.show_migration = true;
                            ui.close();
                        }
                        if ui.button("Saved desktop tabs").clicked() {
                            self.show_desktop_recovery = true;
                            ui.close();
                        }
                        if ui.button("Recover working files").clicked() {
                            self.editors.recovery_open = true;
                            ui.close();
                        }
                        if ui.button("File transfers").clicked() {
                            transfers::show();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !self.workspace.tabs.is_empty(),
                                egui::Button::new("Close all tabs"),
                            )
                            .clicked()
                        {
                            // Tab drops detach desktops and release only their own transports.
                            self.workspace.tabs.clear();
                            self.workspace.selected = None;
                            ui.close();
                        }
                        if ui.button("Import / export").clicked() {
                            self.profile_io = Some(false);
                            ui.close();
                        }
                        ui.separator();
                        ui.label("AetherDesk");
                    });
                    // Reserve the settings/theme controls before laying out the
                    // brand. Optional branding yields space before controls do.
                    let brand_width = (ui.available_width() - 120.0).max(1.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(brand_width, 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if brand_width >= 110.0 {
                                let (logo, _) = ui.allocate_exact_size(
                                    egui::vec2(26.0, 30.0),
                                    egui::Sense::hover(),
                                );
                                self.theme.logo(ui, self.dark, logo);
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("AetherDesk")
                                        .font(theme::heading_font(14.0))
                                        .color(theme::colors(self.dark).heading),
                                )
                                .truncate(),
                            );
                            if brand_width >= 280.0 {
                                ui.label(
                                    egui::RichText::new("SECURE WORKSPACE")
                                        .size(9.0)
                                        .color(theme::colors(self.dark).muted),
                                );
                            }
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("⚙", |ui| {
                            if ui.button("Preferences…").clicked() {
                                self.show_appearance = true;
                                ui.close();
                            }
                            if ui
                                .button(if self.updates.available() {
                                    "Updates · Available…"
                                } else {
                                    "Updates…"
                                })
                                .clicked()
                            {
                                self.updates.open = true;
                                ui.close();
                            }
                            if ui.button("Log files…").clicked() {
                                self.show_logs = true;
                                ui.close();
                            }
                            if ui.button("Help…").clicked() {
                                self.show_help = true;
                                ui.close();
                            }
                            if ui.button("About AetherDesk…").clicked() {
                                self.show_about = true;
                                ui.close();
                            }
                        });
                        if theme::mode_toggle(ui, self.dark) {
                            self.dark = !self.dark;
                            if let Err(error) = self.preferences.toggle_mode(self.dark) {
                                self.report_error(error);
                            }
                            palette(ctx, self.dark);
                            self.persist_settings();
                        }
                    });
                });
            });
        if self.show_desktop_recovery {
            let mut reopen = None;
            let mut recovery_open = self.show_desktop_recovery;
            egui::Window::new("Saved desktop tabs").open(&mut recovery_open).show(ctx, |ui| {
                ui.label("Reopen a duplicate desktop using its previous identity. The host may have ended it; a missing desktop opens a new session.");
                match desktop_recovery::load() {
                    Ok(entries) => {
                        if entries.is_empty() { ui.label("No duplicate desktop tabs have been saved."); }
                        for entry in entries {
                            let profile = self.profiles.iter().find(|p| entry.matches(p)).cloned();
                            ui.horizontal(|ui| {
                                ui.label(format!("{} · {}@{}:{} · {}",entry.name,entry.username,entry.host,entry.port,&entry.desktop_id.to_string()[..8]));
                                if ui.add_enabled(profile.is_some(), egui::Button::new("Reopen")).clicked() {
                                    reopen = profile.clone().map(|p| (p,entry.clone()));
                                }
                                if ui.button("Forget entry").on_hover_text("Remove the recovery bookmark; this does not end the remote desktop").clicked() {
                                    if let Err(error) = desktop_recovery::forget(entry.desktop_id) { self.report_error(error); }
                                }
                            });
                        }
                    }
                    Err(error) => { ui.label(error); }
                }
            });
            self.show_desktop_recovery = recovery_open;
            if let Some((profile, entry)) = reopen {
                if let Some(index) = self
                    .workspace
                    .tabs
                    .iter()
                    .position(|t| t.desktop_identity() == entry.desktop_id)
                {
                    if let Some(old) = self
                        .workspace
                        .selected
                        .and_then(|i| self.workspace.tabs.get_mut(i))
                    {
                        old.deactivate();
                    }
                    self.workspace.selected = Some(index);
                } else {
                    if let Some(old) = self
                        .workspace
                        .selected
                        .and_then(|i| self.workspace.tabs.get_mut(i))
                    {
                        old.deactivate();
                    }
                    self.workspace.open_new(session::SessionTab::recover(
                        profile,
                        entry.desktop_id,
                        entry.username,
                    ));
                }
                self.show_desktop_recovery = false;
            }
        }
        egui::TopBottomPanel::bottom("status")
            .exact_height((self.preferences.settings.font_size + 12.0).max(24.0))
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let status = self
                        .workspace
                        .selected
                        .and_then(|i| self.workspace.tabs.get(i))
                        .map(|t| {
                            format!(
                                "{} · {}",
                                t.profile.name,
                                if t.connected {
                                    "Connected"
                                } else {
                                    "Disconnected"
                                }
                            )
                        })
                        .unwrap_or_else(|| "○  No active session".into());
                    let width = ui.available_width();
                    let clock_width =
                        (ui.text_style_height(&egui::TextStyle::Body) * 5.5).max(72.0);
                    let status_width = (width - clock_width - 12.0).max(1.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(status_width, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let connected =
                                self.workspace.tabs.iter().filter(|t| t.connected).count();
                            let active =
                                self.workspace.tabs.iter().filter(|t| t.is_active()).count();
                            let detail = self
                                .workspace
                                .selected
                                .and_then(|i| self.workspace.tabs.get(i))
                                .map(|tab| {
                                    format!(
                                        "{} · {}",
                                        tab.profile.connection_type.label(),
                                        tab.profile.authentication
                                    )
                                })
                                .unwrap_or_default();
                            let full = format!(
                                "{status} · {connected} connected · {active} active · {detail}\n{}",
                                self.notice
                            );
                            let text = if width > 600.0 {
                                full.replace('\n', " · ")
                            } else {
                                status
                            };
                            ui.add(egui::Label::new(text).truncate())
                                .on_hover_text(full);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let time=self.workspace.selected.and_then(|index|self.workspace.tabs.get(index))
                            .map(|tab|tab.server_time()).unwrap_or_else(||"--:--:--".into());
                        ui.label(time).on_hover_text("Server time from the active connection. Clears after 3 seconds without a reply.");
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                    });
                });
            });
        egui::SidePanel::left("navigation-rail")
            .resizable(false)
            .exact_width(44.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(5, 2)),
            )
            .show(ctx, |ui| {
                self.theme.rail(ui, self.dark);
                egui::ScrollArea::vertical()
                    .id_salt("navigation-rail-scroll")
                    .show(ui, |ui| {
                        let indices = [1, 9, 10, 7, 3, 2, 6, 7];
                        for (section, index) in Sidebar::ALL
                            .into_iter()
                            .zip(indices)
                            .filter(|(section, _)| *section != Sidebar::Logs)
                        {
                            if theme::rail_button(
                                ui,
                                index,
                                section.label(),
                                self.workspace.sidebar_visible && self.workspace.sidebar == section,
                                self.dark,
                            )
                            .clicked()
                            {
                                if self.workspace.sidebar_visible
                                    && self.workspace.sidebar == section
                                {
                                    self.workspace.sidebar_visible = false;
                                } else {
                                    self.workspace.select_sidebar(section);
                                }
                            }
                        }
                    });
            });
        if self.show_appearance {
            self.preferences.show();
            self.show_appearance = false;
        }
        match self.preferences.ui(ctx) {
            Some(preferences::Action::Applied) => {
                self.dark = self.preferences.settings.dark(ctx, self.dark);
                ctx.set_zoom_factor(self.preferences.settings.ui_scale);
                palette(ctx, self.dark);
                self.persist_settings();
            }
            Some(preferences::Action::Assistant) => {
                self.workspace.select_sidebar(Sidebar::Assistant);
                self.assistant.open_preferences();
            }
            Some(preferences::Action::Updates) => self.updates.open = true,
            Some(preferences::Action::Logs) => self.show_logs = true,
            None => {}
        }
        let update_safe = !self.updates.open
            || (self.closing.is_none()
                && !self.workspace.tabs.iter().any(|tab| tab.is_active())
                && self
                    .connections
                    .as_ref()
                    .is_ok_and(|runtime| runtime.is_idle())
                && self.editors.update_ready()
                && !self.assistant.is_busy()
                && !self.tools.is_busy()
                && !transfers::is_busy());
        if self.updates.ui(ctx, update_safe) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.show_logs {
            let mut open = true;
            egui::Window::new("Log files")
                .open(&mut open)
                .fixed_size(log_panel::window_size(ctx.content_rect().size()))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    let hosts = self
                        .workspace
                        .tabs
                        .iter()
                        .filter_map(|t| {
                            t.log_source()
                                .map(|(s, id)| (t.profile.name.clone(), s, id))
                        })
                        .collect();
                    if let Ok(runtime) = &self.connections {
                        self.logs.ui(ui, runtime, hosts);
                    }
                });
            self.show_logs = open;
        }
        if self.show_help {
            let mut open = true;
            egui::Window::new("AetherDesk Help").open(&mut open).resizable(false).show(ctx, |ui| {
                ui.heading("Connections and workspace");
                let keys=&self.preferences.settings.shortcuts;
                ui.label(format!("New connection: {}. Toggle sidebar: {}.",keys.new_connection.label(),keys.sidebar.label()));
                ui.label(format!("Next / previous tab: {} / {}. Drag tabs to reorder.",keys.next_tab.label(),keys.previous_tab.label()));
                ui.label("Close a tab with its × or the middle mouse button.");
                ui.label("Terminal: Ctrl+C interrupts; Ctrl+Shift+C copies; Ctrl+Shift+V pastes.");
                ui.label("Hold Shift to select terminal text when a program captures the mouse.");
                ui.label("Use Terminal settings to add words and choose their colours.");
                ui.label("Passwords are encrypted in Windows Credential Manager; they are excluded from session exports.");
                ui.label("Log files are available from the gear menu.");
            });
            self.show_help = open;
        }
        if self.show_about {
            let mut open = true;
            egui::Window::new("About AetherDesk")
                .open(&mut open)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.heading("AetherDesk");
                    let build = update_manifest::current().map(|v|v[3]).unwrap_or(0);
                    ui.label(format!("Version {} · Build {build}", env!("CARGO_PKG_VERSION")));
                    ui.label(format!("Licence: {}", env!("CARGO_PKG_LICENSE")));
                    ui.label("SSH · SFTP · SSH + X11 · Remote Desktop · AI Assistant");
                    ui.label("A secure workspace for connections, files and AI assistance.");
                    ui.hyperlink_to("Project and downloads", "https://github.com/ashkanr65/AetherDesk");
                    ui.label("Bundled dependency licences are included in THIRD-PARTY-NOTICES.txt and THIRD-PARTY-LICENSES.");
                });
            self.show_about = open;
        }
        if self.show_migration {
            let mut open = true;
            egui::Window::new("Import previous settings").open(&mut open).resizable(false).show(ctx,|ui|{
                let enter = crate::dialog::primary(ui);
                ui.label("Import saved connections, terminal preferences, AI services and chat history from the previous installation. Source files are read only.");
                ui.checkbox(&mut self.migrate_credentials,"Copy saved connection passwords and provider API keys");
                if self.migrate_credentials {ui.label("Master password (only needed for protected credentials)");ui.add(egui::TextEdit::singleline(&mut self.migration_master).password(true));}
                let response = ui.button("Import"); crate::dialog::focus_on_open(ui, &response);
                if response.clicked() || enter {
                    let result=(||->Result<String,String>{
                        let (profiles,profile_secrets,profile_skipped)=migration::import_profiles(self.migrate_credentials,&self.migration_master)?;
                        let existing=self.profiles.iter().map(|p|p.id.clone()).collect::<std::collections::HashSet<_>>();
                        let before=self.profiles.len();self.profiles.extend(profiles.into_iter().filter(|p|!existing.contains(&p.id)));
                        self.persist_profiles();
                        let assistant=self.assistant.import_original(self.migrate_credentials,&self.migration_master)?;
                        Ok(format!("Imported {} connection(s), {} conversation(s), {} provider key(s), {} connection credential(s); {} credential(s) were skipped because they were protected, unavailable, or already exist. Terminal preferences {}.",self.profiles.len()-before,assistant.0,assistant.1,profile_secrets,profile_skipped+assistant.2,if assistant.3{"copied"}else{"already present or unavailable"}))
                    })();
                    self.notice=match result{Ok(s)=>s,Err(e)=>e};
                    self.migration_master.zeroize();
                }
            });
            self.show_migration = open;
        }
        self.workspace_sidebar(ctx);
        process_health::pulse(3);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .inner_margin(if self.workspace.tabs.is_empty() { 0 } else { 8 }),
            )
            .show(ctx, |ui| {
                if !self.workspace.tabs.is_empty() {
                    let tabs = self
                        .workspace
                        .tabs
                        .iter()
                        .map(|t| navigation::Tab {
                            id: t.tab_id(),
                            name: t.profile.name.clone(),
                            detail: format!(
                                "{} · {}:{}",
                                t.profile.connection_type.label(),
                                t.profile.host,
                                t.profile.port
                            ),
                            state: t.state_label(),
                        })
                        .collect::<Vec<_>>();
                    let change = self.tab_strip.ui(ui, &tabs, self.workspace.selected);
                    if let Some(index) = change.select {
                        self.select_tab(index);
                    }
                    if let Some((from, to)) = change.reorder {
                        self.workspace.move_tab(from, to);
                    }
                    if let Some(index) = change.close {
                        // Use identity when reordering and closing happen in one frame.
                        let id = tabs[index].id;
                        if let Some(index) =
                            self.workspace.tabs.iter().position(|t| t.tab_id() == id)
                        {
                            self.workspace.close_tab(index);
                        }
                    }
                    ui.separator();
                }
                if let Some(tab) = self
                    .workspace
                    .selected
                    .and_then(|index| self.workspace.tabs.get_mut(index))
                {
                    match &self.connections {
                        Ok(runtime) => tab.ui(ui, runtime),
                        Err(error) => {
                            ui.label(format!("Connection runtime unavailable: {error}"));
                        }
                    }
                } else {
                    if let Some(kind) = self.theme.home(ui, self.dark) {
                        let mut profile = profile_store::new_profile();
                        profile.connection_type = kind;
                        self.new_profile = Some(profile.into());
                    }
                }
            });
        // Register external desktops for every connection, even when another
        // tab is selected. Registering inside selected-tab UI closes them when
        // the user switches to SSH or SSH + X11 in the main window.
        let mut returned_desktop = None;
        for (index, tab) in self.workspace.tabs.iter_mut().enumerate() {
            if tab.remote_window(ctx) {
                returned_desktop = Some(index);
            }
        }
        if let Some(index) = returned_desktop {
            self.select_tab(index);
            ctx.request_repaint();
        }
        if let Ok(runtime) = &self.connections {
            for tab in &mut self.workspace.tabs {
                tab.files_window(ctx, runtime);
            }
        }
        let action = self
            .new_profile
            .as_mut()
            .and_then(|draft| draft.ui(ctx, self.connections.as_ref().ok(), self.profiles_valid));
        self.finish_connection_form(action);
        if let Some(mut export) = self.profile_io {
            let mut opened = true;
            let mut apply = false;
            egui::Window::new("Session files")
                .open(&mut opened)
                .show(ctx, |ui| {
                    let enter = crate::dialog::primary(ui);
                    ui.checkbox(&mut export, "Export profiles (without credentials)");
                    ui.label("Full JSON file path");
                    let response = ui.text_edit_singleline(&mut self.profile_path);
                    crate::dialog::focus_on_open(ui, &response);
                    ui.label("Export creates a new file; existing files are never overwritten.");
                    apply = ui
                        .button(if export { "Export" } else { "Import" })
                        .clicked()
                        || enter;
                });
            if apply {
                let result = if export {
                    profile_store::export(std::path::Path::new(&self.profile_path), &self.profiles)
                } else {
                    profile_store::import(std::path::Path::new(&self.profile_path))
                        .map(|p| self.profiles.extend(p))
                };
                match result {
                    Ok(()) => {
                        self.notice = "Session file operation completed".into();
                        if !export {
                            self.persist_profiles();
                        }
                    }
                    Err(error) => self.report_error(error),
                }
            }
            self.profile_io = if opened { Some(export) } else { None };
        }
        if self.last_notice != self.notice && !self.notice.is_empty() {
            self.last_notice = self.notice.clone();
            if !self.notice.is_empty() {
                self.notifications.push(notifications::Notice::new(
                    notifications::Kind::Information,
                    self.notice.clone(),
                ));
            }
        }
        self.notifications.ui(ctx, self.dark);
        if let Some(stage) = self.update_health.take() {
            let _ = update_apply::mark_healthy(&stage);
        }
        process_health::pulse(4);
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}
fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("--apply-update") {
        if let Some(path) = std::env::args_os().nth(2) {
            update_apply::entry(std::path::Path::new(&path));
        }
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--r12-ui-check") {
        return r12_ui::run();
    }
    let _update_guard = match update_apply::running_guard() {
        Ok(guard) => guard,
        Err(message) => {
            #[cfg(windows)]
            unsafe {
                let text: Vec<_> = message.encode_utf16().chain(Some(0)).collect();
                let title: Vec<_> = "AetherDesk".encode_utf16().chain(Some(0)).collect();
                windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    0x30,
                );
            }
            return Ok(());
        }
    };
    if let Ok(root) = storage::root() {
        logging::init(&root.join("logs/application.log"));
        crash_diagnostics::init(&root.join("logs"));
    }
    tracing::info!("AetherDesk starting");
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(theme::native_icon())
            .with_maximized(
                preferences::load()
                    .ok()
                    .flatten()
                    .is_some_and(|p| p.start_maximized),
            )
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size(app_layout::MINIMUM)
            .with_clamp_size_to_monitor_size(true),
        ..Default::default()
    };
    graphics::configure(
        &mut options,
        preferences::load()
            .ok()
            .flatten()
            .unwrap_or_default()
            .renderer,
    );
    let _health = storage::root()
        .ok()
        .map(|root| process_health::start(&root.join("logs")));
    eframe::run_native(
        "AetherDesk",
        options,
        Box::new(|creation| {
            process_health::pulse(0);
            graphics::log_adapter(creation);
            remote_keypad::install();
            let app = RustDesktop::default();
            creation.egui_ctx.set_fonts(theme::font_definitions());
            creation
                .egui_ctx
                .set_zoom_factor(app.preferences.settings.ui_scale);
            palette(
                &creation.egui_ctx,
                app.preferences.settings.dark(&creation.egui_ctx, app.dark),
            );
            Ok(Box::new(app))
        }),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_windows_preserve_sidebar_preference_and_restore_width_on_larger_screens() {
        let directory = tempfile::tempdir().unwrap();
        storage::with_test_root(directory.path(), || {
            let mut app = RustDesktop::default();
            let ctx = egui::Context::default();
            app.workspace.sidebar_width = 520.0;
            for width in [1280.0, 480.0, 1280.0, 400.0, 1600.0] {
                for _ in 0..4 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 800.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            app.workspace_sidebar(ctx);
                            egui::CentralPanel::default().show(ctx, |_| {});
                        },
                    );
                }
                assert_eq!(
                    app.workspace.sidebar_width, 520.0,
                    "A temporary resize overwrote the sidebar preference"
                );
                if app_layout::sidebar_maximum(width) >= 180.0 {
                    let state =
                        egui::containers::panel::PanelState::load(&ctx, egui::Id::new("workspace"))
                            .unwrap();
                    let expected = 520.0_f32.min(app_layout::sidebar_maximum(width));
                    assert!(
                        (state.rect.width() - expected).abs() < 1.0,
                        "Sidebar did not refit on width {width}: {:?}",
                        state.rect
                    );
                }
            }
        });
    }
    #[test]
    fn save_and_open_persists_profile_and_never_serializes_the_password() {
        let directory = tempfile::tempdir().unwrap();
        storage::with_test_root(directory.path(), || {
            let mut app = RustDesktop::default();
            let mut profile = profile_store::new_profile();
            profile.host = "fixture.invalid".into();
            profile.username = "fixture-user".into();
            profile.remember_username = true;
            let id = profile.id.clone();
            let mut draft = session_form::Draft::from(profile);
            *draft.secret = "transient-password-for-connection".into();
            app.new_profile = Some(draft);
            app.finish_connection_form(Some(session_form::Action::Open));
            assert!(app.new_profile.is_none());
            assert_eq!(app.workspace.tabs.len(), 1);
            assert_eq!(app.workspace.tabs[0].profile.id, id);
            assert_eq!(profile_store::load().unwrap()[0].id, id);
            let json = std::fs::read_to_string(directory.path().join("sessions.json")).unwrap();
            assert!(!json.contains("transient-password-for-connection"));
        });
    }
    #[test]
    fn application_actions_preserve_workspace_and_reject_closed_connection_identities() {
        let dir = tempfile::tempdir().unwrap();
        storage::with_test_root(dir.path(), || {
            let mut app = RustDesktop::default();
            let ctx = egui::Context::default();
            let profile = SavedSession::from_json(
                &serde_json::json!({"id":"same","name":"session","host":"server"}),
            )
            .unwrap();
            app.open_profile(profile.clone());
            app.open_profile(profile);
            let first = app.workspace.tabs[0].identity();
            let second = app.workspace.tabs[1].identity();
            assert_ne!(first, second);
            assert_ne!(
                app.workspace.tabs[0].tab_id(),
                app.workspace.tabs[1].tab_id()
            );
            app.app_action("app_show_panel", &serde_json::json!({"panel":"Logs"}), &ctx)
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(app.workspace.selected, Some(1));
            assert!(app.show_logs);
            assert_eq!(app.workspace.sidebar, Sidebar::Sessions);
            app.app_action(
                "app_select_connection",
                &serde_json::json!({"connection_id":first}),
                &ctx,
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(app.workspace.selected, Some(0));
            app.workspace.move_tab(0, 1);
            assert_eq!(
                app.workspace.tabs[app.workspace.selected.unwrap()].identity(),
                first
            );
            app.workspace.close_tab(1);
            assert_eq!(app.workspace.tabs[0].identity(), second);
            assert!(app
                .app_action(
                    "app_select_connection",
                    &serde_json::json!({"connection_id":first}),
                    &ctx
                )
                .is_err());
            assert!(app
                .app_action(
                    "app_capture_screen",
                    &serde_json::json!({"connection_id":second}),
                    &ctx
                )
                .is_err());
        });
    }
}
