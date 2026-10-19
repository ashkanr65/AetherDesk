//! Public binary releases; the application never needs a GitHub credential.
use crate::update_manifest::{self as metadata, Asset, Manifest};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};
#[derive(Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    automatic: bool,
    download: bool,
    last_check: i64,
    highest: [u64; 4],
    pending: Option<PathBuf>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            automatic: true,
            download: false,
            last_check: 0,
            highest: [0; 4],
            pending: None,
        }
    }
}
#[derive(Clone)]
struct Candidate {
    manifest: Manifest,
    envelope: Vec<u8>,
    asset_id: u64,
    installer: bool,
}
enum Event {
    Checked(Result<Option<Candidate>, String>),
    Progress(u64, u64),
    Downloaded(Result<PathBuf, String>),
    Restored(Result<Option<(Candidate, PathBuf)>, String>),
}
pub struct Updates {
    prefs: Preferences,
    valid: bool,
    started: Instant,
    status: String,
    error: Option<String>,
    candidate: Option<Candidate>,
    downloaded: Option<PathBuf>,
    receiver: Option<mpsc::Receiver<Event>>,
    progress: Option<(u64, u64)>,
    pub open: bool,
    confirm: bool,
    restored: bool,
    maintenance_done: bool,
}
impl Default for Updates {
    fn default() -> Self {
        let loaded = crate::storage::read_json("updates.json").and_then(|v| {
            v.map(serde_json::from_value)
                .transpose()
                .map_err(|_| "Update settings are damaged and have been preserved".into())
        });
        let valid = loaded.is_ok();
        let error = loaded.as_ref().err().cloned();
        Self {
            prefs: loaded.ok().flatten().unwrap_or_default(),
            valid,
            started: Instant::now(),
            status: String::new(),
            error,
            candidate: None,
            downloaded: None,
            receiver: None,
            progress: None,
            open: false,
            confirm: false,
            restored: false,
            maintenance_done: false,
        }
    }
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("AetherDesk/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(900))
        .https_only(true)
        .build()
        .map_err(|_| "Cannot initialize the update connection".into())
}
fn repository() -> Result<String, String> {
    let repo = metadata::config()?.repository;
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err("Invalid embedded release repository".into());
    }
    Ok(repo)
}
fn request(client: &reqwest::Client, path: &str, binary: bool) -> reqwest::RequestBuilder {
    client
        .get(format!("https://api.github.com{path}"))
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(
            "Accept",
            if binary {
                "application/octet-stream"
            } else {
                "application/vnd.github+json"
            },
        )
}
fn http_error(code: reqwest::StatusCode) -> String {
    match code.as_u16() {
        401 | 403 | 404 => "The public update release is unavailable or has not been published yet. Please try again later.".into(),
        429 => "GitHub is limiting update checks. Please try again later.".into(),
        _ => format!("The update service returned HTTP {}. Try again later.", code.as_u16()),
    }
}
async fn limited(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(http_error(response.status()));
    }
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err("Update response exceeds the size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Update connection interrupted")?
    {
        if bytes.len() + chunk.len() > limit {
            return Err("Update response exceeds the size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[derive(Deserialize)]
struct Release {
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}
#[derive(Deserialize)]
struct ReleaseAsset {
    id: u64,
    name: String,
    size: u64,
}
async fn check(highest: [u64; 4]) -> Result<Option<Candidate>, String> {
    let config = metadata::config()?;
    if config.public_key.is_empty() {
        return Err("This build has no trusted update key. The publisher must configure release signing first.".into());
    }
    let repo = repository()?;
    let client = client()?;
    let response = request(&client, &format!("/repos/{repo}/releases/latest"), false)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "Cannot reach GitHub. Check your connection.")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        // A new public downloads repository can legitimately have no release yet.
        let repository_response = request(&client, &format!("/repos/{repo}"), false)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| "Cannot reach GitHub")?;
        let repository: serde_json::Value =
            serde_json::from_slice(&limited(repository_response, 1024 * 1024).await?)
                .map_err(|_| "Invalid repository response")?;
        if repository["private"] == false {
            return Ok(None);
        }
        return Err("The public update repository is unavailable".into());
    }
    let release: Release = serde_json::from_slice(&limited(response, 1024 * 1024).await?)
        .map_err(|_| "Invalid release response")?;
    if release.draft || release.prerelease {
        return Err("No stable update is published".into());
    }
    let feed = release
        .assets
        .iter()
        .find(|a| a.name == "update.json" && a.size <= metadata::MAX_METADATA as u64)
        .ok_or("This release has no signed Rust update metadata")?;
    let response = request(
        &client,
        &format!("/repos/{repo}/releases/assets/{}", feed.id),
        true,
    )
    .timeout(Duration::from_secs(30))
    .send()
    .await
    .map_err(|_| "Cannot download release metadata")?;
    let envelope = limited(response, metadata::MAX_METADATA).await?;
    let manifest = metadata::verify(
        &envelope,
        &config.public_key,
        chrono::Utc::now().timestamp(),
    )?;
    let candidate_version = metadata::version(&manifest.version, manifest.build)?;
    if candidate_version < highest {
        return Err("An older release was returned. Installation was blocked.".into());
    }
    if candidate_version <= metadata::current()? {
        return Ok(None);
    }
    let exe = std::env::current_exe().map_err(|_| "Cannot locate the application")?;
    let installer = exe
        .parent()
        .is_some_and(|p| p.join("unins000.exe").is_file());
    let asset = if installer {
        manifest
            .installer
            .as_ref()
            .ok_or("This release has no installer update")?
    } else {
        &manifest.portable
    };
    let asset_id = release
        .assets
        .iter()
        .find(|a| a.name == asset.name && a.size == asset.bytes)
        .ok_or("The signed package is missing from this release")?
        .id;
    Ok(Some(Candidate {
        manifest,
        envelope,
        asset_id,
        installer,
    }))
}
fn cache() -> Result<PathBuf, String> {
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("Local application data unavailable")?,
    )
    .join("AetherDesk/updates");
    crate::update_apply::no_links(&root)?;
    std::fs::create_dir_all(&root).map_err(|_| "Cannot create update download folder")?;
    Ok(root)
}
fn discard_stage(stage: &std::path::Path) -> Result<(), String> {
    let root = cache()?
        .canonicalize()
        .map_err(|_| "Cannot inspect update cache")?;
    crate::update_apply::no_links(stage)?;
    let stage = stage
        .canonicalize()
        .map_err(|_| "Cannot inspect staged update")?;
    if stage.parent() != Some(root.as_path())
        || !stage
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("update-"))
    {
        return Err("Refusing to remove a folder outside the update cache".into());
    }
    std::fs::remove_dir_all(stage)
        .map_err(|_| "The update cache is still in use. Try again after restarting.".into())
}
fn clean_completed() {
    if let Ok(root) = cache() {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                if entry.path().join("complete").is_file() {
                    let _ = discard_stage(&entry.path());
                }
            }
        }
    }
}
fn retire_obsolete_download(stage: &std::path::Path) {
    // Installation stages belong to the helper until health acknowledgement.
    if !stage.join("plan.json").exists() {
        let _ = discard_stage(stage);
    }
}
fn restore(stage: PathBuf, highest: [u64; 4]) -> Result<Option<(Candidate, PathBuf)>, String> {
    let root = cache()?
        .canonicalize()
        .map_err(|_| "Cannot inspect update cache")?;
    crate::update_apply::no_links(&stage)?;
    let stage = stage
        .canonicalize()
        .map_err(|_| "The saved update download is missing. Check again for updates.")?;
    if stage.parent() != Some(root.as_path()) {
        return Err("Saved update is outside the update cache".into());
    }
    if stage.join("error.txt").is_file() {
        return Err(format!("The previous update did not finish. Its recovery files were kept in {}. Check again for a fresh download.", stage.display()));
    }
    let envelope =
        std::fs::read(stage.join("update.json")).map_err(|_| "Cannot read saved update")?;
    let manifest = metadata::verify(
        &envelope,
        &metadata::config()?.public_key,
        chrono::Utc::now().timestamp(),
    )?;
    let version = metadata::version(&manifest.version, manifest.build)?;
    if version <= metadata::current()? {
        // On the first updated launch the old helper still needs its plan,
        // journal and backup until it receives our health confirmation.
        // Completed installation stages are removed by delayed maintenance.
        retire_obsolete_download(&stage);
        return Ok(None);
    }
    if version < highest {
        return Err("Saved update is older than the newest verified release".into());
    }
    let installer = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join("unins000.exe").is_file()))
        .unwrap_or(false);
    let candidate = Candidate {
        manifest,
        envelope,
        asset_id: 0,
        installer,
    };
    if installer && candidate.manifest.installer.is_none() {
        return Err("Saved installer update is unavailable".into());
    }
    let asset = selected(&candidate);
    let path = stage.join(&asset.name);
    if std::fs::metadata(&path)
        .map_err(|_| "Saved update package missing")?
        .len()
        != asset.bytes
        || metadata::file_hash(&path)? != asset.sha256.to_ascii_lowercase()
    {
        return Err("The saved update failed verification. Check again for updates.".into());
    }
    Ok(Some((candidate, stage)))
}
fn selected(candidate: &Candidate) -> &Asset {
    if candidate.installer {
        candidate
            .manifest
            .installer
            .as_ref()
            .expect("validated installer")
    } else {
        &candidate.manifest.portable
    }
}
async fn download(
    candidate: Candidate,
    sender: &mpsc::Sender<Event>,
    ctx: &egui::Context,
) -> Result<PathBuf, String> {
    use std::io::Write;
    let repo = repository()?;
    let asset = selected(&candidate);
    let stage = tempfile::Builder::new()
        .prefix("update-")
        .tempdir_in(cache()?)
        .map_err(|_| "Cannot create update stage")?;
    let client = client()?;
    let mut response = request(
        &client,
        &format!("/repos/{repo}/releases/assets/{}", candidate.asset_id),
        true,
    )
    .send()
    .await
    .map_err(|_| "Cannot download the update")?;
    if !response.status().is_success() {
        return Err(http_error(response.status()));
    }
    if response.content_length().is_some_and(|n| n != asset.bytes) {
        return Err("Update package length does not match its signature".into());
    }
    let path = stage.path().join(&asset.name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| "Cannot save update. Check disk space.")?;
    let mut count = 0;
    let mut last = Instant::now();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Download interrupted. Retry the update.")?
    {
        count += chunk.len() as u64;
        if count > asset.bytes {
            return Err("Update download exceeds its declared size".into());
        }
        file.write_all(&chunk)
            .map_err(|_| "Cannot save update. Check disk space.")?;
        if last.elapsed() > Duration::from_millis(150) {
            let _ = sender.send(Event::Progress(count, asset.bytes));
            ctx.request_repaint();
            last = Instant::now();
        }
    }
    file.sync_all()
        .map_err(|_| "Cannot finish update download")?;
    drop(file);
    if count != asset.bytes || metadata::file_hash(&path)? != asset.sha256.to_ascii_lowercase() {
        return Err("Downloaded package verification failed. Nothing was installed.".into());
    }
    std::fs::write(stage.path().join("update.json"), &candidate.envelope)
        .map_err(|_| "Cannot save update metadata")?;
    Ok(stage.keep())
}
impl Updates {
    fn save(&mut self) {
        if self.valid {
            if let Err(e) = crate::storage::write_json(
                "updates.json",
                &serde_json::to_value(&self.prefs).unwrap_or_default(),
            ) {
                self.error = Some(e);
            }
        }
    }
    fn start(&mut self, ctx: &egui::Context, candidate: Option<Candidate>) {
        if self.receiver.is_some() {
            return;
        }
        self.error = None;
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        let highest = self.prefs.highest;
        let ctx = ctx.clone();
        self.status = if candidate.is_some() {
            "Downloading update…"
        } else {
            "Checking for updates…"
        }
        .into();
        if candidate.is_none() {
            self.prefs.last_check = chrono::Utc::now().timestamp();
            self.save();
        }
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let event = match runtime {
                Ok(runtime) => {
                    if let Some(candidate) = candidate {
                        Event::Downloaded(runtime.block_on(download(candidate, &sender, &ctx)))
                    } else {
                        Event::Checked(runtime.block_on(check(highest)))
                    }
                }
                Err(_) => Event::Checked(Err("Cannot start update connection".into())),
            };
            let _ = sender.send(event);
            ctx.request_repaint();
        });
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.restored {
            self.restored = true;
            if let Some(stage) = self.prefs.pending.clone() {
                let (sender, receiver) = mpsc::channel();
                self.receiver = Some(receiver);
                let ctx = ctx.clone();
                let highest = self.prefs.highest;
                std::thread::spawn(move || {
                    let _ = sender.send(Event::Restored(restore(stage, highest)));
                    ctx.request_repaint();
                });
            }
        }
        #[cfg(not(test))]
        if !self.maintenance_done && self.started.elapsed() > Duration::from_secs(10) {
            self.maintenance_done = true;
            std::thread::spawn(clean_completed);
        }
        loop {
            let event = match self.receiver.as_ref().map(|r| r.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.receiver = None;
                    self.progress = None;
                    self.error = Some("The update operation stopped. Please try again.".into());
                    break;
                }
                _ => break,
            };
            match event {
                Event::Progress(done, total) => self.progress = Some((done, total)),
                Event::Checked(result) => {
                    self.receiver = None;
                    match result {
                        Ok(Some(candidate)) => {
                            self.prefs.highest = metadata::version(
                                &candidate.manifest.version,
                                candidate.manifest.build,
                            )
                            .unwrap_or(self.prefs.highest);
                            self.status =
                                format!("AetherDesk {} is available", candidate.manifest.version);
                            self.candidate = Some(candidate);
                            self.save();
                            // A subtle notification is available in Settings; downloading never restarts.
                            if self.prefs.download {
                                self.start(ctx, self.candidate.clone());
                            }
                        }
                        Ok(None) => {
                            self.status = "AetherDesk is up to date".into();
                            self.candidate = None;
                        }
                        Err(error) => {
                            self.status = "Update check did not complete".into();
                            self.error = Some(error);
                        }
                    }
                }
                Event::Downloaded(result) => {
                    self.receiver = None;
                    self.progress = None;
                    match result {
                        Ok(path) => {
                            self.prefs.pending = Some(path.clone());
                            self.save();
                            self.downloaded = Some(path);
                            self.status =
                                "Update downloaded and verified. Ready when you are.".into();
                        }
                        Err(error) => {
                            self.status = "Update download did not complete".into();
                            self.error = Some(error);
                        }
                    }
                }
                Event::Restored(result) => {
                    self.receiver = None;
                    match result {
                        Ok(Some((candidate, path))) => {
                            self.candidate = Some(candidate);
                            self.downloaded = Some(path);
                            self.status = "Verified update ready to install".into();
                        }
                        Ok(None) => {
                            self.prefs.pending = None;
                            self.save();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.prefs.pending = None;
                            self.save();
                        }
                    }
                }
            }
        }
        #[cfg(not(test))]
        if self.valid
            && self.prefs.automatic
            && self.receiver.is_none()
            && self.downloaded.is_none()
            && self.started.elapsed() > Duration::from_secs(10)
            && chrono::Utc::now()
                .timestamp()
                .saturating_sub(self.prefs.last_check)
                >= 86400
        {
            self.start(ctx, None);
        }
    }
    /// Returns true only after confirmation, a safe-work check and helper launch.
    pub fn ui(&mut self, ctx: &egui::Context, safe: bool) -> bool {
        let mut install = false;
        if !self.open {
            return false;
        }
        let mut open = self.open;
        egui::Window::new("Updates").id(egui::Id::new("application-updates")).open(&mut open)
            .resizable(false).default_width(420.0).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.heading(format!("AetherDesk {}", env!("CARGO_PKG_VERSION")));
                ui.label("Stable releases · Installer and portable updates");
                let changed = ui.checkbox(&mut self.prefs.automatic, "Check for updates automatically").changed()
                    | ui.checkbox(&mut self.prefs.download, "Download updates automatically").changed();
                ui.small("Checks at startup, then daily. Installing always asks before restarting.");
                if changed { self.save(); }
                if self.prefs.last_check > 0 {
                    if let Some(time) = chrono::DateTime::from_timestamp(self.prefs.last_check, 0) {
                        ui.small(format!("Last checked: {}", time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")));
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.receiver.is_none() && self.downloaded.is_none(), egui::Button::new("Check now")).clicked() { self.start(ctx, None); }
                    if self.receiver.is_some() { ui.spinner(); }
                });
                ui.label(&self.status);
                if let Some((done,total)) = self.progress { ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).show_percentage()); }
                if let Some(error) = &self.error { ui.colored_label(ui.visuals().error_fg_color, error); }
                if let Some(candidate) = &self.candidate {
                    egui::ScrollArea::vertical().id_salt("update-release-notes").max_height(140.0).show(ui, |ui| { ui.label(&candidate.manifest.notes); });
                    if self.downloaded.is_none() && ui.add_enabled(self.receiver.is_none(), egui::Button::new("Download update")).clicked() { self.start(ctx, self.candidate.clone()); }
                }
                if self.downloaded.is_some() {
                    if !safe { ui.small("Disconnect sessions, finish transfers and agent/tool work, and save editor changes before installing."); }
                    if ui.add_enabled(safe, egui::Button::new("Update and restart…")).clicked() { self.confirm = true; }
                    if ui.button("Discard download").clicked() {
                        if let Some(stage) = &self.downloaded {
                            match discard_stage(stage) {
                                Ok(()) => { self.downloaded = None; self.candidate = None; self.prefs.pending = None; self.save(); self.status = "Downloaded update removed".into(); self.confirm = false; },
                                Err(error) => self.error = Some(error),
                            }
                        }
                    }
                }
            });
        self.open = open;
        if !open {
            self.confirm = false;
        }
        if self.confirm {
            egui::Window::new("Install update?")
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label("AetherDesk will close, install the verified update, and restart.");
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(safe, egui::Button::new("Update and restart"))
                            .clicked()
                        {
                            if let (Some(stage), Some(candidate)) =
                                (&self.downloaded, &self.candidate)
                            {
                                match crate::update_apply::stage_apply(stage, candidate.installer) {
                                    Ok(()) => {
                                        install = true;
                                        self.confirm = false;
                                    }
                                    Err(error) => {
                                        self.error = Some(error);
                                        self.confirm = false;
                                    }
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm = false;
                        }
                    });
                });
        }
        install
    }
    pub fn available(&self) -> bool {
        self.candidate.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_updated_launch_preserves_the_helpers_recovery_files() {
        let stage = tempfile::tempdir().unwrap();
        for name in ["plan.json", "rollback.json", "previous-executable"] {
            std::fs::write(stage.path().join(name), b"needed by the running helper").unwrap();
        }
        retire_obsolete_download(stage.path());
        for name in ["plan.json", "rollback.json", "previous-executable"] {
            assert_eq!(
                std::fs::read(stage.path().join(name)).unwrap(),
                b"needed by the running helper"
            );
        }
        assert!(!stage.path().join("complete").exists());
    }
    #[test]
    fn update_defaults_preserve_consent_version_floor_and_exclude_credentials() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let mut updates = Updates::default();
            assert!(updates.prefs.automatic);
            assert!(!updates.prefs.download);
            updates.prefs.highest = [1, 2, 3, 4];
            updates.save();
            let bytes = std::fs::read_to_string(dir.path().join("updates.json")).unwrap();
            assert!(!bytes.contains("never-in-settings"));
            assert_eq!(Updates::default().prefs.highest, [1, 2, 3, 4]);
            std::fs::write(dir.path().join("updates.json"), b"broken").unwrap();
            let mut damaged = Updates::default();
            damaged.save();
            assert_eq!(
                std::fs::read(dir.path().join("updates.json")).unwrap(),
                b"broken"
            );
        });
    }
}
