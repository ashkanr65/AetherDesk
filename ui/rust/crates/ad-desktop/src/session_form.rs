//! Transient form secrets never enter SavedSession or the JSON profile store.
use ad_desktop_model::profiles::{ConnectionType, SavedSession};
use eframe::egui;
use tokio::sync::mpsc;
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
pub enum Action {
    Save,
    Open,
    Cancel,
}
pub struct Draft {
    pub profile: SavedSession,
    pub secret: Zeroizing<String>,
    pub master: Zeroizing<String>,
    original: SavedSession,
    existing_credential: bool,
    pending: Option<mpsc::Receiver<Result<(), String>>>,
    picker: Option<mpsc::Receiver<Result<Option<String>, String>>>,
    pending_action: Action,
    error: Option<String>,
}
impl From<SavedSession> for Draft {
    fn from(profile: SavedSession) -> Self {
        let existing_credential = profile.remember_password
            && crate::vault::read(&format!("ssh-{}", profile.id))
                .ok()
                .flatten()
                .is_some();
        Self {
            existing_credential,
            original: profile.clone(),
            profile,
            secret: Zeroizing::new(String::new()),
            master: Zeroizing::new(String::new()),
            pending: None,
            picker: None,
            pending_action: Action::Save,
            error: None,
        }
    }
}
impl Draft {
    fn keeps_existing_secret(&self) -> bool {
        let a = &self.original;
        let b = &self.profile;
        self.existing_credential
            && a.remember_password
            && b.remember_password
            && a.host == b.host
            && a.port == b.port
            && a.username == b.username
            && a.authentication == b.authentication
            && a.key_path == b.key_path
            && a.master_password_protected == b.master_password_protected
    }
    fn validate(&self) -> Result<(), String> {
        crate::profile_store::validate(&self.profile)?;
        if self.profile.remember_password {
            if self.profile.username.trim().is_empty() {
                return Err("Enter the username for the saved credential".into());
            }
            if self.secret.is_empty()
                && !self.keeps_existing_secret()
                && self.profile.authentication != "sshKey"
            {
                return Err("Enter the password to save it".into());
            }
            if self.profile.master_password_protected
                && self.master.is_empty()
                && !(self.secret.is_empty() && self.keeps_existing_secret())
            {
                return Err("Enter a master password to protect this credential".into());
            }
        }
        Ok(())
    }
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        runtime: Option<&crate::connection::Connections>,
        can_save: bool,
    ) -> Option<Action> {
        if let Some(receiver) = &mut self.picker {
            match receiver.try_recv() {
                Ok(Ok(Some(path))) => {
                    self.profile.wireguard_config_path = Some(path);
                    self.profile.wireguard_enabled = true;
                    self.picker = None;
                }
                Ok(Ok(None)) => self.picker = None,
                Ok(Err(error)) => {
                    self.error = Some(error);
                    self.picker = None;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.error = Some("The file picker stopped".into());
                    self.picker = None;
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(40))
                }
            }
        }
        if let Some(receiver) = &mut self.pending {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    self.pending = None;
                    self.existing_credential = true;
                    return Some(self.pending_action);
                }
                Ok(Err(error)) => {
                    self.pending = None;
                    self.error = Some(error);
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("Credential saving did not complete".into());
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(40));
                }
            }
        }
        let mut open = true;
        let mut action = None;
        let busy = self.pending.is_some() || self.picker.is_some();
        let title = if self.original.host.is_empty() {
            "New connection"
        } else {
            "Edit connection"
        };
        // A pending credential write cannot be dismissed halfway through saving.
        let mut window = egui::Window::new(title)
            .id(egui::Id::new("connection-form"))
            .default_width(510.0)
            .resizable(false);
        if !busy {
            window = window.open(&mut open);
        }
        window.show(ctx, |ui| {
            let enter = !busy && crate::dialog::primary(ui);
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-160.0).max(180.0)).show(ui, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    egui::Grid::new("connection-fields").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                        let p = &mut self.profile;
                        ui.label("Name"); let response = ui.text_edit_singleline(&mut p.name);
                        crate::dialog::focus_on_open(ui, &response); ui.end_row();
                        ui.label("Host"); ui.text_edit_singleline(&mut p.host); ui.end_row();
                        ui.label("Port"); ui.add(egui::DragValue::new(&mut p.port).range(1..=65535)); ui.end_row();
                        ui.label("Username"); ui.text_edit_singleline(&mut p.username); ui.end_row();
                        ui.label("Connection");
                        egui::ComboBox::from_id_salt("connection-kind").selected_text(p.connection_type.label()).show_ui(ui, |ui| {
                            for kind in [ConnectionType::SshTerminal, ConnectionType::SftpOnly, ConnectionType::SshX11, ConnectionType::RemoteDesktop] {
                                ui.selectable_value(&mut p.connection_type, kind, kind.label());
                            }
                        }); ui.end_row();
                        ui.label("Authentication");
                        egui::ComboBox::from_id_salt("authentication-kind").selected_text(if p.authentication == "sshKey" { "SSH private key" } else { "Password" }).show_ui(ui, |ui| {
                            ui.selectable_value(&mut p.authentication, "password".into(), "Password");
                            ui.selectable_value(&mut p.authentication, "sshKey".into(), "SSH private key");
                        }); ui.end_row();
                        if p.authentication == "sshKey" {
                            ui.label("Private key path"); ui.text_edit_singleline(p.key_path.get_or_insert_with(String::new)); ui.end_row();
                        }
                        ui.label(if p.authentication == "sshKey" { "Key passphrase" } else { "Password" });
                        ui.add(egui::TextEdit::singleline(&mut *self.secret).password(true).hint_text(if self.existing_credential { "Saved (leave blank to keep)" } else { "Enter password / passphrase" })); ui.end_row();
                        ui.label("WireGuard (this session)");
                        ui.horizontal(|ui| {
                            if ui.add(egui::TextEdit::singleline(p.wireguard_config_path.get_or_insert_with(String::new)).hint_text("Optional .conf file").desired_width(230.0)).changed() {
                                p.wireguard_enabled=p.wireguard_config_path.as_ref().is_some_and(|p|!p.trim().is_empty());
                            }
                            if ui.button("Browse").clicked() {
                                let (sender,receiver)=mpsc::channel(1);self.picker=Some(receiver);
                                let context=ctx.clone();
                                std::thread::spawn(move|| {let _=sender.blocking_send(crate::file_picker::wireguard());context.request_repaint();});
                            }
                        }); ui.end_row();
                        if p.wireguard_config_path.as_ref().is_some_and(|p|!p.trim().is_empty()) {
                            // Separate grid row: align the path with the label,
                            // then put enable/remove directly beneath that path.
                            ui.label("");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut p.wireguard_enabled,"Enable for this session only");
                                if crate::ui_icons::button(ui,crate::ui_icons::Icon::Remove,"Remove WireGuard configuration",false).clicked() {
                                    p.wireguard_config_path=None; p.wireguard_enabled=false;
                                }
                            }); ui.end_row();
                        }
                        if p.connection_type == ConnectionType::RemoteDesktop {
                            ui.label("Desktop");
                            egui::ComboBox::from_id_salt("desktop-kind").selected_text(&p.desktop_environment).show_ui(ui, |ui| {
                                for name in ["xfce", "kde", "gnome", "mate", "cinnamon", "lxqt"] {
                                    ui.selectable_value(&mut p.desktop_environment, name.to_owned(), name);
                                }
                            }); ui.end_row();
                        }
                    });
                    ui.separator();
                    ui.checkbox(&mut self.profile.remember_username, "Remember username");
                    ui.checkbox(&mut self.profile.remember_password, "Save password / key passphrase securely");
                    if self.profile.remember_password {
                        self.profile.remember_username = true;
                        ui.checkbox(&mut self.profile.master_password_protected, "Protect saved secret with a master password");
                        if self.profile.master_password_protected {
                            ui.horizontal(|ui| { ui.label("Master password"); ui.add(egui::TextEdit::singleline(&mut *self.master).password(true)); });
                        }
                    } else { self.profile.master_password_protected = false; }
                    ui.label("Saved passwords are encrypted for your Windows account, outside the session file.");
                    ui.horizontal(|ui| {
                        if ui.add_enabled(can_save, egui::Button::new("Save connection")).clicked() || (enter && can_save) { action = Some(Action::Save); }
                        if ui.add_enabled(can_save, egui::Button::new("Save and open")).clicked() { action = Some(Action::Open); }
                        if ui.button("Cancel").clicked() { action = Some(Action::Cancel); }
                    });
                });
                if busy { ui.horizontal(|ui| { ui.spinner(); ui.label("Saving encrypted credential…"); }); }
                if let Some(error) = &self.error { ui.colored_label(ui.visuals().error_fg_color, error); }
            });
        });
        if !open {
            return Some(Action::Cancel);
        }
        if matches!(action, Some(Action::Save | Action::Open)) {
            if let Err(error) = self.validate() {
                self.error = Some(error);
                return None;
            }
        }
        if matches!(action, Some(Action::Save | Action::Open)) {
            let keep = self.secret.is_empty() && self.keeps_existing_secret();
            if self.profile.remember_password && !keep {
                let Some(runtime) = runtime else {
                    self.error = Some("Connection runtime is unavailable".into());
                    return None;
                };
                let p = self.profile.clone();
                let secret = self.secret.clone();
                let master = self.master.clone();
                let (tx, rx) = mpsc::channel(1);
                self.pending = Some(rx);
                self.pending_action = action.unwrap_or(Action::Save);
                let ctx = ctx.clone();
                runtime.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::credentials::remember(&p, &p.username, &secret, &master)
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|v| v);
                    let _ = tx.send(result).await;
                    ctx.request_repaint();
                });
                return None;
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_and_open_waits_for_credential_completion_and_keeps_its_action() {
        let mut draft = Draft::from(crate::profile_store::new_profile());
        let (sender, receiver) = mpsc::channel(1);
        draft.pending = Some(receiver);
        draft.pending_action = Action::Open;
        sender.blocking_send(Ok(())).unwrap();
        assert!(matches!(
            draft.ui(&egui::Context::default(), None, true),
            Some(Action::Open)
        ));
        assert!(draft.existing_credential);
        assert!(draft.pending.is_none());
    }
    #[test]
    fn form_secrets_are_transient_and_identity_changes_require_a_new_password() {
        let mut p = crate::profile_store::new_profile();
        p.host = "server".into();
        p.username = "user".into();
        p.remember_password = true;
        let mut draft = Draft::from(p);
        draft.existing_credential = true; // Model an existing encrypted vault entry.
        assert!(draft.keeps_existing_secret());
        draft.profile.host = "other-server".into();
        assert!(draft.validate().is_err());
        *draft.secret = "private-test-password".into();
        assert!(draft.validate().is_ok());
        let json = draft.profile.to_json().to_string();
        assert!(!json.contains("private-test-password"));
        draft.profile.master_password_protected = true;
        assert!(draft.validate().is_err());
        *draft.master = "master-test-password".into();
        assert!(draft.validate().is_ok());
        assert!(!draft
            .profile
            .to_json()
            .to_string()
            .contains("master-test-password"));
    }
}
