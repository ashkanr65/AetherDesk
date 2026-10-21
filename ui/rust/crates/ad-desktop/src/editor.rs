//! Native document windows and pinned remote working copies.
use ad_desktop_model::files::{safe_local_name, TextFile, MAX_EDITABLE_BYTES};
use ad_ssh::{FileProgress, RemoteFileMetadata, RemoteFileRequest, SshSession};
use eframe::egui;
use std::{io::Write, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

#[derive(Clone)]
pub enum Location {
    Local(PathBuf),
    Remote {
        session: Arc<SshSession>,
        path: String,
    },
}
pub struct OpenFile {
    pub location: Location,
    pub force_text: bool,
    pub action: OpenAction,
}
#[derive(Clone, Default)]
pub enum OpenAction {
    #[default]
    Default,
    ChooseProgram,
    DefaultProgram,
    Compare(Location),
    Recover(crate::working_copies::Record),
}
struct Loaded {
    editable: bool,
    bytes: Vec<u8>,
    metadata: Option<RemoteFileMetadata>,
    directory: Option<tempfile::TempDir>,
    path: PathBuf,
    recovery: Option<crate::working_copies::Record>,
    baseline: Option<Vec<u8>>,
}
enum Reply {
    Opened(Result<Loaded, String>),
    Launched(Result<bool, String>),
    Reloaded(Result<Vec<u8>, String>),
    Backup(Result<(), String>),
    Saved {
        result: Result<Option<RemoteFileMetadata>, String>,
        bytes: Vec<u8>,
    },
    ExternalSaved {
        result: Result<Option<RemoteFileMetadata>, String>,
        stamp: Option<(u64, std::time::SystemTime)>,
        baseline: Option<Vec<u8>>,
    },
}
struct Document {
    id: uuid::Uuid,
    location: Location,
    title: String,
    force_text: bool,
    file: Option<TextFile>,
    text: String,
    saved: Vec<u8>,
    metadata: Option<RemoteFileMetadata>,
    directory: Option<tempfile::TempDir>,
    path: Option<PathBuf>,
    receiver: mpsc::Receiver<Reply>,
    sender: mpsc::Sender<Reply>,
    busy: bool,
    closing: bool,
    closed: bool,
    error: Option<String>,
    tools: crate::editor_tools::Tools,
    external: bool,
    external_stamp: Option<(u64, std::time::SystemTime)>,
    language: crate::editor_language::Language,
    cursor: usize,
    action: OpenAction,
    recovery: Option<crate::working_copies::Record>,
    backup_pending: bool,
    backup_dirty: bool,
    backup_at: std::time::Instant,
    observed_stamp: Option<(u64, std::time::SystemTime)>,
    external_changed: bool,
    recovered_dirty: bool,
    save_after_backup: bool,
}
#[derive(Default)]
pub struct Editors {
    documents: Vec<Document>,
    comparisons: Vec<crate::comparison::Comparison>,
    pub recovery_open: bool,
    review: Option<WorkspaceReview>,
    navigation: Option<(uuid::Uuid, serde_json::Value)>,
}
struct WorkspaceTarget {
    id: uuid::Uuid,
    edits: Vec<serde_json::Value>,
    lsp_version: Option<u64>,
    before: Option<String>,
    after: Option<String>,
}
struct WorkspaceReview {
    source: uuid::Uuid,
    version: u64,
    targets: Vec<WorkspaceTarget>,
    error: Option<String>,
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
fn same_location(a: &Location, b: &Location) -> bool {
    match (a, b) {
        (Location::Local(a), Location::Local(b)) => std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b),
        (
            Location::Remote {
                session: a,
                path: pa,
            },
            Location::Remote {
                session: b,
                path: pb,
            },
        ) => a.connection_id() == b.connection_id() && pa == pb,
        _ => false,
    }
}
fn language_location(source: &Location, uri: &str) -> Result<Location, String> {
    let url = reqwest::Url::parse(uri).map_err(|e| e.to_string())?;
    if url.scheme() != "file"
        || url.host_str().is_some_and(|h| !h.is_empty())
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Language navigation requires an absolute file URI on the original computer".into(),
        );
    }
    match source {
        Location::Local(_) => Ok(Location::Local(
            url.to_file_path().map_err(|_| "Invalid local file URI")?,
        )),
        Location::Remote { session, .. } => {
            let input = url.path().as_bytes();
            let mut decoded = Vec::new();
            let mut i = 0;
            while i < input.len() {
                if input[i] == b'%' {
                    if i + 2 >= input.len() {
                        return Err("Invalid file URI escape".into());
                    }
                    let hex =
                        std::str::from_utf8(&input[i + 1..i + 3]).map_err(|e| e.to_string())?;
                    decoded.push(u8::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
                    i += 3;
                } else {
                    decoded.push(input[i]);
                    i += 1;
                }
            }
            let path = String::from_utf8(decoded).map_err(|e| e.to_string())?;
            if !path.starts_with('/') || path.contains('\0') {
                return Err("Invalid remote file path".into());
            }
            Ok(Location::Remote {
                session: session.clone(),
                path,
            })
        }
    }
}
async fn load(location: Location) -> Result<Loaded, String> {
    load_with_recovery(location, true).await
}
pub(crate) async fn read_comparison(location: Location) -> Result<Vec<u8>, String> {
    let loaded = load_with_recovery(location, false).await?;
    if !loaded.editable {
        return Err("Compare regular files up to 8 MiB".into());
    }
    Ok(loaded.bytes)
}
async fn load_with_recovery(location: Location, keep: bool) -> Result<Loaded, String> {
    match location {
        Location::Local(path) => {
            let path = tokio::fs::canonicalize(path)
                .await
                .map_err(|e| e.to_string())?;
            let metadata = tokio::fs::metadata(&path)
                .await
                .map_err(|e| e.to_string())?;
            if !metadata.is_file() {
                return Err("Choose a regular file".into());
            }
            let editable = metadata.len() <= MAX_EDITABLE_BYTES as u64;
            let bytes = if editable {
                tokio::fs::read(&path).await.map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            Ok(Loaded {
                editable,
                bytes,
                metadata: None,
                directory: None,
                path,
                recovery: None,
                baseline: None,
            })
        }
        Location::Remote { session, path } => {
            if session.is_closed() {
                return Err("Reconnect this file's SSH session before opening it".into());
            }
            let metadata = session
                .file_operation(
                    request("stat", path.clone()),
                    Arc::new(FileProgress::default()),
                )
                .await
                .map_err(|e| e.to_string())?
                .metadata
                .ok_or("File metadata missing")?;
            if metadata.kind != "file" || metadata.size.unwrap_or(u64::MAX) > 512 * 1024 * 1024 {
                return Err(
                    "External working copies support regular files up to 512 MiB; transfer larger files using the file panes."
                        .into(),
                );
            }
            let root = crate::working_copies::root()?;
            let directory = tempfile::Builder::new()
                .prefix(&uuid::Uuid::new_v4().to_string())
                .rand_bytes(0)
                .tempdir_in(root)
                .map_err(|e| e.to_string())?;
            let local = directory.path().join(safe_local_name(
                path.rsplit('/').next().unwrap_or("document"),
            ));
            let mut operation = request("download", path);
            operation.local_path = Some(local.to_string_lossy().into_owned());
            let result = session
                .file_operation(operation, Arc::new(FileProgress::default()))
                .await
                .map_err(|e| e.to_string())?;
            let editable = tokio::fs::metadata(&local)
                .await
                .map_err(|e| e.to_string())?
                .len()
                <= MAX_EDITABLE_BYTES as u64;
            let bytes = if editable {
                tokio::fs::read(&local).await.map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            if bytes.len() > MAX_EDITABLE_BYTES {
                return Err("The file grew beyond the editor size limit".into());
            }
            let metadata = result.metadata.ok_or("Host revision missing")?;
            let recovery = if keep {
                Some(crate::working_copies::create(
                    &session,
                    metadata.clone(),
                    &local,
                    &bytes,
                )?)
            } else {
                None
            };
            let directory = if keep {
                let _ = directory.keep();
                None
            } else {
                Some(directory)
            };
            Ok(Loaded {
                editable,
                bytes,
                metadata: Some(metadata),
                directory,
                path: local,
                recovery,
                baseline: None,
            })
        }
    }
}
async fn save(
    location: Location,
    path: PathBuf,
    metadata: Option<RemoteFileMetadata>,
    before: Vec<u8>,
    bytes: Vec<u8>,
) -> Result<Option<RemoteFileMetadata>, String> {
    match location {
        Location::Local(_) => tokio::task::spawn_blocking(move || {
            if std::fs::read(&path).map_err(|e| e.to_string())? != before {
                return Err(
                    "Conflict: the file changed on disk. Your edits have been kept.".into(),
                );
            }
            let mut stage =
                tempfile::NamedTempFile::new_in(path.parent().ok_or("No parent directory")?)
                    .map_err(|e| e.to_string())?;
            stage
                .as_file()
                .set_permissions(
                    std::fs::metadata(&path)
                        .map_err(|e| e.to_string())?
                        .permissions(),
                )
                .map_err(|e| e.to_string())?;
            stage.write_all(&bytes).map_err(|e| e.to_string())?;
            stage.as_file().sync_all().map_err(|e| e.to_string())?;
            if std::fs::read(&path).map_err(|e| e.to_string())? != before {
                return Err("Conflict: the file changed while saving".into());
            }
            stage.persist(&path).map_err(|e| e.error.to_string())?;
            Ok(None)
        })
        .await
        .map_err(|e| e.to_string())?,
        Location::Remote { session, .. } => {
            if session.is_closed() {
                return Err(
                    "The original SSH session is closed. Your unsaved edits have been kept.".into(),
                );
            }
            let metadata = metadata.ok_or("Host file identity is missing")?;
            if metadata.revision.is_none() {
                return Err("Host revision is missing; save refused".into());
            }
            // Use a new immutable working file for each save, even if the user
            // keeps typing or an external editor changes the visible copy.
            let stage = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
            tokio::fs::write(stage.path(), &bytes)
                .await
                .map_err(|e| e.to_string())?;
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(|e| e.to_string())?;
            let mut operation = request("save", metadata.canonical_path.clone());
            operation.expected_revision = metadata.revision;
            operation.local_path = Some(stage.path().to_string_lossy().into_owned());
            let result = session
                .file_operation(operation, Arc::new(FileProgress::default()))
                .await
                .map_err(|e| e.to_string())?;
            Ok(result.metadata)
        }
    }
}
impl Editors {
    pub fn update_ready(&self) -> bool {
        self.documents
            .iter()
            .all(|d| !d.busy && !d.backup_pending && !d.dirty())
    }
    pub(crate) fn diagnostic_window(&self) -> Option<(egui::ViewportId, bool)> {
        self.documents
            .iter()
            .find(|document| !document.closed)
            .map(|document| {
                (
                    egui::ViewportId::from_hash_of(document.id),
                    document.file.is_some() && !document.busy,
                )
            })
    }
    pub fn recovery_ui(
        &mut self,
        ctx: &egui::Context,
        runtime: &crate::connection::Connections,
        sessions: &[Arc<SshSession>],
    ) {
        if !self.recovery_open {
            return;
        }
        let mut open = true;
        let mut recover = None;
        egui::Window::new("Recover working files").open(&mut open).default_width(650.0).show(ctx,|ui|{
            ui.label("Copies and editor drafts stay on this computer. Uploading always requires the original host/account and an unchanged server revision.");
            match crate::working_copies::list(){Ok(records)=>{if records.is_empty(){ui.label("No saved working files.");}for record in records{
                ui.horizontal(|ui|{ui.label(format!("{}:{} · {}",record.host,record.port,record.metadata.canonical_path));let session=sessions.iter().find(|s|record.matches(s));
                    let already_open=self.documents.iter().any(|d|d.recovery.as_ref().is_some_and(|r|r.id==record.id));
                    if ui.add_enabled(session.is_some()&&!already_open,egui::Button::new("Recover")).clicked(){recover=session.map(|s|(s.clone(),record.clone()));}
                    if ui.button("Copy local path").clicked(){if let Ok(path)=record.path(){ui.ctx().copy_text(path.display().to_string());}}
                });
            }},Err(error)=>{ui.label(error);}}
        });
        self.recovery_open = open;
        if let Some((session, record)) = recover {
            let path = record.metadata.canonical_path.clone();
            self.open(
                OpenFile {
                    location: Location::Remote { session, path },
                    force_text: false,
                    action: OpenAction::Recover(record),
                },
                runtime,
                ctx,
            );
            self.recovery_open = false;
        }
    }
    pub fn open(
        &mut self,
        mut file: OpenFile,
        runtime: &crate::connection::Connections,
        ctx: &egui::Context,
    ) {
        if !file.force_text
            && matches!(file.action, OpenAction::Default)
            && crate::preferences::current().system_editor
        {
            file.action = OpenAction::DefaultProgram;
        }
        if let OpenAction::Compare(other) = file.action.clone() {
            self.comparisons.push(crate::comparison::Comparison::new(
                file.location,
                other,
                runtime,
                ctx,
            ));
            return;
        }
        if let Some(existing) = self.documents.iter_mut().find(|d| {
            !d.closed
                && match (&d.location, &file.location) {
                    (Location::Local(a), Location::Local(b)) => std::fs::canonicalize(a)
                        .ok()
                        .zip(std::fs::canonicalize(b).ok())
                        .is_some_and(|(a, b)| a == b),
                    (
                        Location::Remote {
                            session: a,
                            path: pa,
                        },
                        Location::Remote {
                            session: b,
                            path: pb,
                        },
                    ) => {
                        a.connection_id() == b.connection_id()
                            && (pa == pb
                                || d.metadata.as_ref().is_some_and(|m| m.canonical_path == *pb))
                    }
                    _ => false,
                }
        }) {
            if !existing.busy && !existing.backup_pending && !existing.dirty() {
                if matches!(file.action, OpenAction::ChooseProgram) {
                    existing.choose_program(runtime, ctx);
                }
                if matches!(file.action, OpenAction::DefaultProgram) {
                    existing.launch_default(runtime, ctx);
                }
            }
            ctx.send_viewport_cmd_to(
                egui::ViewportId::from_hash_of(existing.id),
                egui::ViewportCommand::Focus,
            );
            return;
        }
        let title = match &file.location {
            Location::Local(path) => path.display().to_string(),
            Location::Remote { path, .. } => path.clone(),
        };
        let (sender, receiver) = mpsc::channel(4);
        let location = file.location.clone();
        let action = file.action.clone();
        let tx = sender.clone();
        let context = ctx.clone();
        runtime.spawn(async move {
            let result = if let OpenAction::Recover(record)=action {
                async {
                    let Location::Remote{session,..}=location else{return Err("Recovery requires the original host connection".into());};
                    if !record.matches(&session){return Err("Connect the original host/account with the same verified key to recover this copy".into());}
                    let path=record.path()?;let size=tokio::fs::metadata(&path).await.map_err(|e|e.to_string())?.len();
                    let editable=size<=MAX_EDITABLE_BYTES as u64;
                    let bytes=if editable{match record.read_internal(".draft")?{Some(bytes)=>bytes,None=>tokio::fs::read(&path).await.map_err(|e|e.to_string())?}}else{vec![]};
                    Ok(Loaded{editable,bytes,metadata:Some(record.metadata.clone()),directory:None,path,baseline:Some(record.baseline()?),recovery:Some(record)})
                }.await
            }else{load(location).await};
            let _ = tx.send(Reply::Opened(result)).await;
            context.request_repaint();
        });
        let id = uuid::Uuid::new_v4();
        crate::file_workspace::protect(id, &file.location, None);
        self.documents.push(Document {
            id,
            location: file.location,
            title: title.clone(),
            force_text: file.force_text,
            file: None,
            text: String::new(),
            saved: vec![],
            metadata: None,
            directory: None,
            path: None,
            receiver,
            sender,
            busy: true,
            closing: false,
            closed: false,
            error: None,
            tools: crate::editor_tools::Tools::new(&title),
            external: false,
            external_stamp: None,
            language: crate::editor_language::Language::for_path(&title),
            cursor: 0,
            action: file.action,
            recovery: None,
            backup_pending: false,
            backup_dirty: false,
            backup_at: std::time::Instant::now(),
            observed_stamp: None,
            external_changed: false,
            recovered_dirty: false,
            save_after_backup: false,
        });
    }
    fn language_action(
        &mut self,
        source: uuid::Uuid,
        action: crate::editor_language::Action,
        runtime: &crate::connection::Connections,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let document = self
            .documents
            .iter()
            .find(|d| d.id == source)
            .ok_or("Source editor closed")?;
        let location = document.location.clone();
        let version = document.language.revision();
        match action {
            crate::editor_language::Action::Definition { uri, range } => {
                let location = language_location(&location, &uri)?;
                self.open(
                    OpenFile {
                        location: location.clone(),
                        force_text: true,
                        action: OpenAction::Default,
                    },
                    runtime,
                    ctx,
                );
                let id = self
                    .documents
                    .iter()
                    .find(|d| d.matches_location(&location))
                    .ok_or("Definition could not be opened")?
                    .id;
                self.navigation = Some((id, range));
            }
            crate::editor_language::Action::Edit {
                workspace,
                version: request_version,
            } => {
                if self.review.is_some() {
                    return Err("Review or dismiss the pending language edit first".into());
                }
                if version != request_version {
                    return Err("Document changed while the language request was running".into());
                }
                let mut changes = Vec::new();
                if let Some(entries) = workspace["changes"].as_object() {
                    for (uri, edits) in entries {
                        changes.push((
                            uri.clone(),
                            edits.as_array().ok_or("Invalid workspace edits")?.clone(),
                            None,
                        ));
                    }
                }
                if let Some(entries) = workspace["documentChanges"].as_array() {
                    for entry in entries {
                        if !entry["kind"].is_null() {
                            return Err("Language server file creation, rename or deletion is not supported by this text-edit review".into());
                        }
                        let uri = entry["textDocument"]["uri"]
                            .as_str()
                            .ok_or("Missing document URI")?
                            .to_owned();
                        let raw_version = &entry["textDocument"]["version"];
                        if !raw_version.is_null() && raw_version.as_u64().is_none() {
                            return Err("Invalid document version".into());
                        }
                        changes.push((
                            uri,
                            entry["edits"]
                                .as_array()
                                .ok_or("Missing document edits")?
                                .clone(),
                            raw_version.as_u64(),
                        ));
                    }
                }
                if changes.is_empty() {
                    return Ok(());
                }
                if changes.len() > 64 {
                    return Err("Workspace edit exceeds 64 documents".into());
                }
                // Validate every URI and edit before opening any target. Only files on
                // the original authenticated host (or local computer) can be selected.
                let mut locations = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for (uri, edits, version) in changes {
                    if !seen.insert(uri.clone()) || edits.len() > 100_000 {
                        return Err("Duplicate target or excessive language edits".into());
                    }
                    locations.push((language_location(&location, &uri)?, edits, version));
                }
                let mut targets = Vec::new();
                for (location, edits, lsp_version) in locations {
                    self.open(
                        OpenFile {
                            location: location.clone(),
                            force_text: true,
                            action: OpenAction::Default,
                        },
                        runtime,
                        ctx,
                    );
                    let target = self
                        .documents
                        .iter()
                        .find(|d| d.matches_location(&location))
                        .ok_or("Language target could not be opened")?;
                    targets.push(WorkspaceTarget {
                        id: target.id,
                        edits,
                        lsp_version,
                        before: None,
                        after: None,
                    });
                }
                self.review = Some(WorkspaceReview {
                    source,
                    version,
                    targets,
                    error: None,
                });
            }
        }
        Ok(())
    }
    fn review_ui(&mut self, ctx: &egui::Context, runtime: &crate::connection::Connections) {
        let Some(mut review) = self.review.take() else {
            return;
        };
        // Complete loads before taking a snapshot; existing unsaved buffers are
        // preserved and used as the baseline. Save conflict checks stay in force.
        let source_valid = self
            .documents
            .iter()
            .any(|d| d.id == review.source && !d.closed && d.language.revision() == review.version);
        if !source_valid {
            review.error = Some(
                "Source document changed or closed. Dismiss and request the edits again.".into(),
            );
        }
        let mut ready = true;
        for target in &mut review.targets {
            let Some(document) = self
                .documents
                .iter_mut()
                .find(|d| d.id == target.id && !d.closed)
            else {
                review.error = Some("A target editor closed".into());
                continue;
            };
            document.poll();
            if document.busy || document.backup_pending {
                ready = false;
                continue;
            }
            if document.external || document.file.is_none() || document.external_changed {
                review.error = Some(
                    "A target is unavailable, externally modified, or not an editable text file"
                        .into(),
                );
                continue;
            }
            if target.id != review.source && document.dirty() {
                review.error = Some("Save the other target documents before requesting a multi-file language edit; their unsaved text is preserved.".into());
                continue;
            }
            if let Some(before) = &target.before {
                if &document.text != before {
                    review.error = Some(
                        "A target changed during review. Dismiss and request the edits again."
                            .into(),
                    );
                }
                continue;
            }
            if target
                .lsp_version
                .is_some_and(|v| v != document.language.revision())
            {
                review.error =
                    Some("Language server edit refers to an outdated document version".into());
                continue;
            }
            match crate::editor_tools::lsp_edits(&document.text, &target.edits) {
                Ok(after) => {
                    if let Err(error) = document.file.as_ref().unwrap().encode(&after) {
                        review.error = Some(error.into());
                        continue;
                    }
                    target.before = Some(document.text.clone());
                    target.after = Some(after);
                }
                Err(error) => review.error = Some(error),
            }
        }
        let mut apply = false;
        let mut dismiss = false;
        let id = egui::ViewportId::from_hash_of((review.source, "language-edit-review"));
        ctx.show_viewport_immediate(
            id,
            crate::app_layout::builder(
                ctx,
                id,
                "AetherDesk · Review language edits",
                egui::vec2(900.0, 500.0),
            ),
            |ctx, _| {
                crate::app_layout::update(ctx, "edit-review");
                if ctx.input(|i| i.viewport().close_requested()) {
                    dismiss = true;
                    return;
                }
                egui::TopBottomPanel::bottom("review-actions").show(ctx, |ui| {
                    if let Some(error) = &review.error {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    ui.horizontal_wrapped(|ui| {
                        apply = ui
                            .add_enabled(
                                ready
                                    && review.error.is_none()
                                    && review.targets.iter().all(|t| t.after.is_some()),
                                egui::Button::new("Apply all edits"),
                            )
                            .clicked();
                        dismiss = ui.button("Dismiss").clicked();
                    });
                });
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.label(
                    "Apply these changes to the editor buffers? Save each file when you are ready.",
                );
                    if !ready {
                        ui.spinner();
                        ui.label("Loading target files…");
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    }
                    egui::ScrollArea::vertical()
                        .max_height(ui.available_height().max(1.0))
                        .show(ui, |ui| {
                            for target in &review.targets {
                                if let Some(document) =
                                    self.documents.iter().find(|d| d.id == target.id)
                                {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(format!(
                                            "{} · {} edits",
                                            document.title,
                                            target.edits.len()
                                        ));
                                        if let (Some(before), Some(after)) =
                                            (&target.before, &target.after)
                                        {
                                            if ui.button("Compare").clicked() {
                                                self.comparisons.push(
                                                    crate::comparison::Comparison::text(
                                                        document.title.clone() + " · Language edit",
                                                        before.clone(),
                                                        after.clone(),
                                                        runtime,
                                                        ctx,
                                                    ),
                                                );
                                            }
                                        }
                                    });
                                }
                            }
                        });
                });
            },
        );
        if apply {
            for target in review.targets {
                if let Some(document) = self.documents.iter_mut().find(|d| d.id == target.id) {
                    document.tools.observe(&document.text);
                    document.text = target.after.unwrap();
                    document.tools.observe(&document.text);
                    document.language.changed();
                    document.backup_dirty = true;
                }
            }
        } else if !dismiss {
            self.review = Some(review)
        }
    }
    pub fn can_close(&mut self) -> bool {
        let mut ready = true;
        for document in &mut self.documents {
            if document.busy || document.backup_pending || document.dirty() {
                document.closing = true;
                ready = false;
            }
        }
        ready
    }
    pub fn ui(&mut self, ctx: &egui::Context, runtime: &crate::connection::Connections) {
        self.review_ui(ctx, runtime);
        let mut actions = Vec::new();
        let mut diffs = Vec::new();
        let windows = self
            .documents
            .iter()
            .filter(|d| !d.closed)
            .map(|d| (d.id, d.title.clone()))
            .collect::<Vec<_>>();
        self.comparisons.retain_mut(|comparison| comparison.ui(ctx));
        for document in &mut self.documents {
            document.poll();
            document.backup(runtime, ctx);
            if document.closed {
                continue;
            }
            let title = format!(
                "{}{} · AetherDesk Editor",
                if document.dirty() { "● " } else { "" },
                document.title
            );
            let id = egui::ViewportId::from_hash_of(document.id);
            ctx.show_viewport_immediate(id,crate::app_layout::builder(ctx,id,title,egui::vec2(1000.0,720.0)),|ctx,_|{
                crate::app_layout::update(ctx,"editor");
                if ctx.input(|i|i.viewport().close_requested()) {
                    if !document.request_close(){ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);}else{return;}
                }
                let mut save_now=false;
                let controls_height=(ctx.content_rect().height()*0.4).max(1.0);
                egui::TopBottomPanel::top("editor-actions").max_height(controls_height).show(ctx,|ui| {
                    egui::ScrollArea::vertical().id_salt(("editor-controls-scroll",document.id)).max_height(controls_height).auto_shrink([false,true]).show(ui,|ui| {
                    ui.horizontal_wrapped(|ui|{for (id,title) in &windows{if ui.selectable_label(*id==document.id,title.rsplit(['/', '\\']).next().unwrap_or(title)).clicked(){ctx.send_viewport_cmd_to(egui::ViewportId::from_hash_of(*id),egui::ViewportCommand::Focus);}}});
                    ui.horizontal_wrapped(|ui| {
                        if !document.external && document.file.is_some() && ui.button("Compare with saved").clicked(){if let Some(saved)=TextFile::decode(&document.saved){diffs.push(crate::comparison::Comparison::text(document.title.clone()+" · Saved ↔ Editor",saved.text,document.text.clone(),runtime,ctx));}}
                        save_now=ui.add_enabled(!document.busy&&((!document.external&&document.file.is_some())||(document.external&&matches!(document.location,Location::Remote{..}))),egui::Button::new(if document.external {if matches!(document.location,Location::Remote{..}){"Upload saved working copy"}else{"Save in local application"}}else{"Save"})).clicked();
                        if ui.button("Find / replace").clicked(){document.tools.find=!document.tools.find;}
                        if ui.button("Language service").clicked(){document.language.toggle();}
                        if ui.add_enabled(document.path.is_some()&&!document.busy&&!document.backup_pending&&!document.dirty(),egui::Button::new("Open with default program")).clicked(){document.launch_default(runtime,ctx);}
                        if document.busy{ui.spinner();ui.label("Working…");}
                        if ui.add_enabled(document.path.is_some()&&!document.busy&&!document.backup_pending&&!document.dirty(),egui::Button::new("Open with…")).clicked(){document.choose_program(runtime,ctx);}
                        if document.external_changed&&!document.external {
                            ui.label("The working file changed externally; your editor text has been preserved.");
                            if ui.button("Reload working file (discard editor text)").clicked(){if let Some(path)=document.path.clone(){let tx=document.sender.clone();document.busy=true;runtime.spawn(async move{let result=async{if tokio::fs::metadata(&path).await.map_err(|e|e.to_string())?.len()>MAX_EDITABLE_BYTES as u64{return Err("File exceeds the editor limit".into());}tokio::fs::read(path).await.map_err(|e|e.to_string())}.await;let _=tx.send(Reply::Reloaded(result)).await;});}}
                        }
                    });
                    if !document.external && document.file.is_some() {
                        let before=document.tools.revision();
                        document.tools.toolbar(ui,&mut document.text);
                        document.tools.search_ui(ui,&mut document.text,document.language.revision());
                        if before!=document.tools.revision() {document.language.changed();document.backup_dirty=true;}
                    }
                    if let Some(error)=&document.error {ui.colored_label(egui::Color32::LIGHT_RED,error);}
                    });
                });
                if !document.external {
                    let remote=match (&document.location,&document.metadata){(Location::Remote{session,..},Some(metadata))=>Some((session.clone(),metadata.canonical_path.clone())),_=>None};
                    egui::TopBottomPanel::bottom("editor-language").max_height((ctx.content_rect().height()*0.25).max(1.0)).show(ctx,|ui|{egui::ScrollArea::vertical().id_salt(("editor-language-scroll",document.id)).max_height((ctx.content_rect().height()*0.25).max(1.0)).auto_shrink([false,true]).show(ui,|ui|{let previous=document.language.revision();let focused=document.tools.has_focus(ctx);document.language.ui(ui,runtime,remote,document.path.clone(),&mut document.text,document.cursor,document.tools.width,document.tools.tabs,focused);if previous!=document.language.revision(){document.backup_dirty=true;document.tools.observe(&document.text);}});});
                }
                egui::CentralPanel::default().show(ctx,|ui| {
                    if document.external {ui.label(if matches!(document.location,Location::Remote{..}){"Editing with a Windows application. Save there, then upload the working copy here to update the server file."}else{"Editing the local file with a Windows application. Save changes in that application."});if let Some(path)=&document.path{ui.add(egui::Label::new(path.display().to_string()).selectable(true));}ctx.request_repaint_after(std::time::Duration::from_secs(1));}
                    else if document.file.is_some(){let before=document.tools.revision();document.cursor=document.tools.code(ui,&mut document.text,document.language.revision(),runtime);if before!=document.tools.revision(){document.language.changed();document.backup_dirty=true;}}
                    else if !document.busy {ui.label("This file is not editable as text. Use your local program to open its working copy.");}
                });
                if ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL,egui::Key::S)){save_now=true;}
                if document.closing {
                    egui::Window::new("Unsaved changes").collapsible(false).resizable(false).show(ctx,|ui| {
                        let enter = crate::dialog::primary(ui);
                        ui.label("Save changes before closing this editor?");
                        ui.horizontal(|ui|{
                            let response=ui.add_enabled(!document.busy,egui::Button::new("Save")); crate::dialog::focus_on_open(ui,&response);
                            if response.clicked() || (enter && !document.busy){save_now=true;}
                            if ui.add_enabled(!document.busy&&!document.backup_pending,egui::Button::new("Discard")).clicked(){if !document.external{if let Some(record)=&document.recovery{if let Ok(path)=record.path(){let _=std::fs::remove_file(path.parent().unwrap().join(".draft"));}}}document.closed=true;}
                            if ui.button("Cancel").clicked(){document.closing=false;}
                        });
                    });
                }
                if save_now&&!document.busy {document.save(runtime,ctx);}
                if document.closed {ctx.send_viewport_cmd(egui::ViewportCommand::Close);}
            });
            while let Some(action) = document.language.take_action() {
                actions.push((document.id, action));
            }
        }
        self.comparisons.extend(diffs);
        for (source, action) in actions {
            if let Err(error) = self.language_action(source, action, runtime, ctx) {
                if let Some(document) = self.documents.iter_mut().find(|d| d.id == source) {
                    document.error = Some(error);
                }
            }
        }
        if let Some((id, range)) = self.navigation.clone() {
            if let Some(document) = self.documents.iter_mut().find(|d| d.id == id && !d.busy) {
                if document.file.is_some() {
                    match (
                        crate::editor_tools::utf16_offset(&document.text, &range["start"]),
                        crate::editor_tools::utf16_offset(&document.text, &range["end"]),
                    ) {
                        (Ok(a), Ok(b)) => document.tools.select(a..b),
                        _ => {
                            document.error =
                                Some("Definition position is outside the document".into())
                        }
                    }
                }
                self.navigation = None;
            }
        }
        self.documents.retain(|d| !d.closed);
    }
}
impl Document {
    fn request_close(&mut self) -> bool {
        if self.busy || self.backup_pending || self.dirty() {
            self.closing = true;
            false
        } else {
            self.closed = true;
            true
        }
    }
    fn matches_location(&self, location: &Location) -> bool {
        same_location(&self.location, location)
            || match (&self.location, location, &self.metadata) {
                (
                    Location::Remote { session: a, .. },
                    Location::Remote { session: b, path },
                    Some(metadata),
                ) => a.connection_id() == b.connection_id() && metadata.canonical_path == *path,
                _ => false,
            }
    }
    fn launch_default(&mut self, runtime: &crate::connection::Connections, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else {
            return;
        };
        self.busy = true;
        self.action = OpenAction::Default;
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || open_external(&path).map(|_| true))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            let _ = tx.send(Reply::Launched(result)).await;
            ctx.request_repaint();
        });
    }
    fn choose_program(&mut self, runtime: &crate::connection::Connections, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        self.busy = true;
        runtime.spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || crate::file_workspace::open_with(&path))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
            let _ = tx.send(Reply::Launched(result)).await;
            ctx.request_repaint();
        });
    }
    fn backup(&mut self, runtime: &crate::connection::Connections, ctx: &egui::Context) {
        if self.save_after_backup && !self.backup_pending {
            self.save_after_backup = false;
            self.save(runtime, ctx);
        }
        if self.action_is_choose() && !self.busy && self.path.is_some() {
            self.action = OpenAction::Default;
            self.choose_program(runtime, ctx);
        }
        if matches!(self.action, OpenAction::DefaultProgram) && !self.busy && self.path.is_some() {
            self.launch_default(runtime, ctx);
        }
        if self.backup_pending || !self.backup_dirty || self.external {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        if self.backup_at.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        let (Some(record), Some(file)) = (&self.recovery, &self.file) else {
            return;
        };
        let bytes = match file.encode(&self.text) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let record = record.clone();
        let tx = self.sender.clone();
        let ctx = ctx.clone();
        self.backup_pending = true;
        self.backup_dirty = false;
        self.backup_at = std::time::Instant::now();
        runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let path = record.path()?.parent().unwrap().join(".draft");
                crate::working_copies::atomic(&path, &bytes)
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = tx.send(Reply::Backup(result)).await;
            ctx.request_repaint();
        });
    }
    fn action_is_choose(&self) -> bool {
        matches!(self.action, OpenAction::ChooseProgram)
    }
    fn dirty(&self) -> bool {
        if self.external {
            return matches!(self.location, Location::Remote { .. })
                && (self.recovered_dirty
                    || self
                        .path
                        .as_ref()
                        .is_some_and(|p| file_stamp(p) != self.external_stamp));
        }
        self.file.as_ref().is_some_and(|file| {
            file.encode(&self.text)
                .map_or(true, |bytes| bytes != self.saved)
        })
    }
    fn poll(&mut self) {
        while let Ok(reply) = self.receiver.try_recv() {
            if !matches!(&reply, Reply::Backup(_)) {
                self.busy = false;
            }
            match reply {
                Reply::Backup(result) => {
                    self.backup_pending = false;
                    if let Err(error) = result {
                        self.error = Some(format!("Draft recovery could not be updated: {error}"));
                    }
                }
                Reply::Launched(result) => match result {
                    Ok(true) => {
                        self.language.stop();
                        self.external = true;
                    }
                    Ok(false) => {}
                    Err(error) => self.error = Some(error),
                },
                Reply::Reloaded(result) => match result {
                    Ok(bytes) => {
                        if let Some(file) = TextFile::decode(&bytes) {
                            self.text = file.text.clone();
                            self.tools.observe(&self.text);
                            self.file = Some(file);
                            self.external_changed = false;
                            self.observed_stamp = self.path.as_ref().and_then(|p| file_stamp(p));
                            self.backup_dirty = true;
                            self.language.changed();
                        } else {
                            self.error = Some(
                                "Changed file is binary or malformed; editor text was kept".into(),
                            );
                        }
                    }
                    Err(error) => self.error = Some(error),
                },
                Reply::Opened(Ok(loaded)) => {
                    self.file = if loaded.editable {
                        TextFile::decode(&loaded.bytes)
                    } else {
                        None
                    };
                    self.saved = loaded.baseline.unwrap_or(loaded.bytes);
                    self.recovery = loaded.recovery;
                    self.metadata = loaded.metadata;
                    self.directory = loaded.directory;
                    self.path = Some(loaded.path);
                    self.external_stamp = self.path.as_ref().and_then(|p| file_stamp(p));
                    self.observed_stamp = self.external_stamp;
                    self.recovered_dirty = matches!(self.action, OpenAction::Recover(_));
                    crate::file_workspace::protect(
                        self.id,
                        &self.location,
                        self.metadata.as_ref().map(|m| m.canonical_path.as_str()),
                    );
                    if let Some(file) = &self.file {
                        self.text = file.text.clone();
                        self.tools.observe(&self.text);
                    } else if self.force_text && !self.action_is_choose() {
                        self.error = Some(
                            "This is binary, malformed text, or too large for the editor".into(),
                        );
                    } else if !self.action_is_choose() {
                        self.action = OpenAction::DefaultProgram;
                    }
                }
                Reply::Opened(Err(error)) => {
                    crate::file_workspace::release(self.id);
                    self.error = Some(error);
                    self.action = OpenAction::Default;
                }
                Reply::Saved {
                    result: Ok(metadata),
                    bytes,
                } => {
                    self.metadata = metadata;
                    self.saved = bytes;
                    let recovery_error = if let (Some(record), Some(metadata)) =
                        (&mut self.recovery, &self.metadata)
                    {
                        record.advance(metadata.clone(), Some(&self.saved)).err()
                    } else {
                        None
                    };
                    self.language.saved(&self.text);
                    self.external_stamp = self.path.as_ref().and_then(|p| file_stamp(p));
                    self.observed_stamp = self.external_stamp;
                    self.error = recovery_error;
                    if !self.dirty() {
                        if let Some(record) = &self.recovery {
                            if let Ok(path) = record.path() {
                                let draft = path.parent().unwrap().join(".draft");
                                let _ = std::fs::remove_file(draft);
                            }
                        }
                    }
                    if self.closing && !self.dirty() {
                        self.closed = true;
                    }
                }
                Reply::Saved {
                    result: Err(error), ..
                } => {
                    self.error = Some(error);
                    self.closing = false;
                }
                Reply::ExternalSaved {
                    result,
                    stamp,
                    baseline,
                } => match result {
                    Ok(metadata) => {
                        self.metadata = metadata;
                        self.external_stamp = stamp;
                        self.recovered_dirty = false;
                        self.error = if let (Some(record), Some(metadata)) =
                            (&mut self.recovery, &self.metadata)
                        {
                            record
                                .advance_baseline(metadata.clone(), baseline.as_deref())
                                .err()
                        } else {
                            None
                        };
                        if self.closing && !self.dirty() {
                            self.closed = true;
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.closing = false;
                    }
                },
            }
        }
        if !self.external && !self.busy && !self.backup_pending {
            if let Some(path) = &self.path {
                self.external_changed |=
                    self.observed_stamp.is_some() && file_stamp(path) != self.observed_stamp;
            }
        }
    }
    fn save(&mut self, runtime: &crate::connection::Connections, ctx: &egui::Context) {
        if self.backup_pending {
            self.save_after_backup = true;
            return;
        }
        if !self.external && self.external_changed {
            self.error=Some("The working file changed outside this editor. Reload it or compare the changes before saving.".into());
            self.closing = false;
            return;
        }
        if self.external {
            let Some(path) = self.path.clone() else {
                return;
            };
            let Location::Remote { session, .. } = self.location.clone() else {
                if self.closing {
                    self.closed = true;
                }
                return;
            };
            let Some(metadata) = self.metadata.clone() else {
                self.error = Some("Host file identity missing".into());
                return;
            };
            let tx = self.sender.clone();
            let ctx = ctx.clone();
            let stamp = file_stamp(&path);
            self.busy = true;
            runtime.spawn(async move {
                let result = async {
                    if metadata.revision.is_none() {
                        return Err("Host revision is missing; upload refused".into());
                    }
                    let stage = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
                    tokio::fs::copy(&path, stage.path())
                        .await
                        .map_err(|e| e.to_string())?;
                    if file_stamp(&path) != stamp {
                        return Err(
                            "The local program is still changing this file; save it and retry"
                                .into(),
                        );
                    }
                    let mut operation = request("save", metadata.canonical_path);
                    operation.expected_revision = metadata.revision;
                    operation.local_path = Some(stage.path().to_string_lossy().into_owned());
                    let baseline = if tokio::fs::metadata(stage.path())
                        .await
                        .map_err(|e| e.to_string())?
                        .len()
                        <= MAX_EDITABLE_BYTES as u64
                    {
                        Some(
                            tokio::fs::read(stage.path())
                                .await
                                .map_err(|e| e.to_string())?,
                        )
                    } else {
                        None
                    };
                    let metadata = session
                        .file_operation(operation, Arc::new(FileProgress::default()))
                        .await
                        .map(|r| r.metadata)
                        .map_err(|e| e.to_string())?;
                    Ok((metadata, baseline))
                }
                .await;
                let (result, baseline) = match result {
                    Ok((metadata, bytes)) => (Ok(metadata), bytes),
                    Err(error) => (Err(error), None),
                };
                let _ = tx
                    .send(Reply::ExternalSaved {
                        result,
                        stamp,
                        baseline,
                    })
                    .await;
                ctx.request_repaint();
            });
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let Some(path) = self.path.clone() else {
            return;
        };
        let bytes = match file.encode(&self.text) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(error.into());
                return;
            }
        };
        let (location, metadata, before, tx, context) = (
            self.location.clone(),
            self.metadata.clone(),
            self.saved.clone(),
            self.sender.clone(),
            ctx.clone(),
        );
        self.busy = true;
        runtime.spawn(async move {
            let result = save(location, path, metadata, before, bytes.clone()).await;
            let _ = tx.send(Reply::Saved { result, bytes }).await;
            context.request_repaint();
        });
    }
}
pub(crate) fn check_external_file(path: &std::path::Path) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if [
        "exe", "com", "bat", "cmd", "ps1", "msi", "lnk", "url", "vbs", "js", "scr", "hta",
    ]
    .contains(&extension.as_str())
    {
        return Err("Executable files are not launched from the file editor. Save the file and explicitly choose how to run it.".into());
    }
    Ok(())
}
fn open_external(path: &std::path::Path) -> Result<(), String> {
    check_external_file(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                window: isize,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> isize;
        }
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
        let code = unsafe {
            ShellExecuteW(
                0,
                operation.as_ptr(),
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if code <= 32 {
            return Err("No local application could open this file. Configure its Windows file association.".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Local file associations are currently supported on Windows".into())
    }
}
fn file_stamp(path: &std::path::Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}
impl Drop for Document {
    fn drop(&mut self) {
        crate::file_workspace::release(self.id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_close_is_owned_by_the_document_and_dirty_or_busy_work_blocks_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(directory.path(), || {
            let path = directory.path().join("document.txt");
            std::fs::write(&path, b"saved").unwrap();
            let runtime = crate::connection::Connections::new().unwrap();
            let ctx = egui::Context::default();
            let mut editors = Editors::default();
            let mut layout_time = 0.0;
            for _ in 0..3 {
                editors.open(
                    OpenFile {
                        location: Location::Local(path.clone()),
                        force_text: true,
                        action: OpenAction::Default,
                    },
                    &runtime,
                    &ctx,
                );
                let index = editors.documents.len() - 1;
                assert!(!editors.documents[index].request_close()); // Still loading.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while editors.documents[index].busy {
                    editors.documents[index].poll();
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                assert!(editors.documents[index].file.is_some());
                editors.documents[index].closing = false;
                editors.documents[index].text.push_str(" changed");
                let edited = editors.documents[index].text.clone();
                for (width, height, dpi) in [
                    (1000.0, 720.0, 1.0),
                    (480.0, 340.0, 2.0),
                    (800.0, 560.0, 1.5),
                    (1000.0, 720.0, 1.0),
                ] {
                    for _ in 0..4 {
                        layout_time += 0.25;
                        let mut input = egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, height),
                            )),
                            time: Some(layout_time),
                            ..Default::default()
                        };
                        let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
                        viewport.inner_rect = input.screen_rect;
                        viewport.monitor_size = Some(egui::vec2(1920.0 / dpi, 1080.0 / dpi));
                        viewport.native_pixels_per_point = Some(dpi);
                        let _ = ctx.run(input, |ctx| {
                            crate::app_layout::update(ctx, "editor-fixture");
                            egui::CentralPanel::default().show(ctx, |ui| {
                                let document = &mut editors.documents[index];
                                document.tools.code(
                                    ui,
                                    &mut document.text,
                                    document.language.revision(),
                                    &runtime,
                                );
                            });
                        });
                    }
                    assert_eq!(editors.documents[index].text, edited);
                    assert!(editors.documents[index].dirty());
                    assert_eq!(std::fs::read(&path).unwrap(), b"saved");
                }
                assert!(!editors.can_close());
                assert!(!editors.documents[index].request_close());
                assert!(!editors.documents[index].closed);
                editors.documents[index].text = "saved".into();
                editors.documents[index].closing = false;
                assert!(editors.documents[index].request_close());
                assert!(editors.can_close());
            }
            assert_eq!(editors.documents.len(), 3);
            assert!(editors.documents.iter().all(|d| d.closed));
            assert_eq!(std::fs::read(&path).unwrap(), b"saved");
        });
    }
    #[test]
    fn local_language_navigation_never_accepts_network_or_non_file_uris() {
        let source = Location::Local(std::path::PathBuf::from("C:/work/source.rs"));
        for uri in [
            "https://example.com/code",
            "file://server/share/file",
            "file:///C:/work/a.rs?command=run",
            "file:///C:/work/a.rs#fragment",
        ] {
            assert!(language_location(&source, uri).is_err());
        }
    }
    #[tokio::test]
    async fn local_conflict_preserves_both_user_edits_and_external_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file");
        std::fs::write(&path, b"external").unwrap();
        assert!(save(
            Location::Local(path.clone()),
            path.clone(),
            None,
            b"before".to_vec(),
            b"edited".to_vec()
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"external");
    }
}
