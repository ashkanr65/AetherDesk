//! Durable, account-bound transfer queue. Pausing retains private stages; retry
//! keeps the original source/destination revisions rather than approving edits.
use ad_ssh::{
    FileProgress, RemoteFileMetadata, RemoteFileRequest, ResumableTransferFile, SshSession,
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use tokio::sync::mpsc;
use uuid::Uuid;
const MAX_JOBS: usize = 256;
const MAX_ENTRIES: usize = 100_000;
const CONCURRENCY: usize = 4;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Queued,
    Preparing,
    Running,
    Paused,
    Failed,
    Complete,
    Cancelled,
}
impl State {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Preparing => "Scanning",
            Self::Running => "Transferring",
            Self::Paused => "Paused",
            Self::Failed => "Failed",
            Self::Complete => "Completed",
            Self::Cancelled => "Cancelled",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Job {
    id: Uuid,
    account: String,
    host: String,
    upload: bool,
    local: String,
    remote: String,
    directory: bool,
    replace: bool,
    state: State,
    error: Option<String>,
    // Older clients reject unknown queue fields. Read the short-lived Build 6
    // inline format, but write progress to a separate, non-authoritative file.
    #[serde(default, skip_serializing)]
    progress: ProgressSnapshot,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ProgressSnapshot {
    done: u64,
    total: Option<u64>,
    files_done: u64,
    files_total: u64,
    #[serde(default)]
    skipped_links: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plan {
    id: Uuid,
    account: String,
    local_root: String,
    remote_root: String,
    directories: Vec<Directory>,
    files: Vec<ResumableTransferFile>,
    #[serde(default, skip_serializing)]
    skipped_links: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Directory {
    local: String,
    remote: String,
}
struct Progress {
    file: Arc<FileProgress>,
    completed: Mutex<(u64, u64)>,
    total: AtomicU64,
    files_total: AtomicU64,
    planned: AtomicBool,
    skipped_links: AtomicU64,
}
impl Progress {
    fn new() -> Self {
        Self {
            file: Arc::new(FileProgress::default()),
            completed: Mutex::new((0, 0)),
            total: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            planned: AtomicBool::new(false),
            skipped_links: AtomicU64::new(0),
        }
    }
    fn snapshot(&self) -> ProgressSnapshot {
        let planned = self.planned.load(Ordering::Acquire);
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        let total = self.total.load(Ordering::Relaxed);
        ProgressSnapshot {
            done: completed
                .0
                .saturating_add(self.file.transferred.load(Ordering::Relaxed))
                .min(total),
            total: planned.then_some(total),
            files_done: completed.1,
            files_total: self.files_total.load(Ordering::Relaxed),
            skipped_links: self.skipped_links.load(Ordering::Relaxed),
        }
    }
    fn finish_file(&self, size: u64) {
        // Switching files must not briefly count the previous file twice.
        let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        self.file.transferred.store(0, Ordering::Relaxed);
        completed.0 = completed.0.saturating_add(size);
        completed.1 += 1;
    }
}
struct Active {
    progress: Arc<Progress>,
    reply: mpsc::Receiver<Result<(), String>>,
    cleanup: bool,
}
struct Queue {
    jobs: Vec<Job>,
    active: HashMap<Uuid, Active>,
    valid: bool,
    error: Option<String>,
    show: bool,
    changed: u64,
    dirty: bool,
}
static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
fn destination_lock(key: String) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}
fn queue() -> &'static Mutex<Queue> {
    QUEUE.get_or_init(|| {
        let loaded = crate::storage::read_json("transfer-queue.json").and_then(|v| match v {
            Some(v) => serde_json::from_value::<Vec<Job>>(v)
                .map_err(|_| "Damaged transfer queue preserved".into()),
            None => Ok(vec![]),
        });
        let (mut jobs, valid, error) = match loaded {
            Ok(jobs) if jobs.len() <= MAX_JOBS && jobs.iter().all(valid_job) => (jobs, true, None),
            Ok(_) => (
                vec![],
                false,
                Some("Invalid transfer queue preserved".into()),
            ),
            Err(e) => (vec![], false, Some(e)),
        };
        restore_progress(&mut jobs);
        for job in &mut jobs {
            if matches!(job.state, State::Queued | State::Preparing | State::Running) {
                job.state = State::Paused;
                job.error = Some(
                    "Recovered after restart. Connect the original host and choose Resume.".into(),
                );
            }
        }
        Mutex::new(Queue {
            jobs,
            active: HashMap::new(),
            valid,
            error,
            show: false,
            changed: 0,
            dirty: valid,
        })
    })
}
fn valid_job(j: &Job) -> bool {
    !j.id.is_nil()
        && j.account.len() == 64
        && j.account.bytes().all(|b| b.is_ascii_hexdigit())
        && Path::new(&j.local).is_absolute()
        && j.remote.starts_with('/')
        && !j.remote.contains('\0')
        && j.local.len() < 32768
        && j.remote.len() < 32768
}
fn restore_progress(jobs: &mut [Job]) {
    let progress = crate::storage::read_json("transfer-progress.json")
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<HashMap<Uuid, ProgressSnapshot>>(value).ok())
        .filter(|progress| progress.len() <= MAX_JOBS);
    if let Some(progress) = progress {
        for job in jobs {
            if let Some(snapshot) = progress.get(&job.id).filter(|p| {
                p.files_total <= MAX_ENTRIES as u64
                    && p.skipped_links <= MAX_ENTRIES as u64
                    && p.files_done <= p.files_total
                    && p.total.is_none_or(|total| p.done <= total)
            }) {
                job.progress = snapshot.clone();
            }
        }
    }
}
fn save(q: &mut Queue) -> Result<(), String> {
    if !q.valid {
        return Err("Transfer queue is damaged; it has been preserved".into());
    }
    if !q.dirty {
        return Ok(());
    }
    crate::storage::write_json(
        "transfer-queue.json",
        &serde_json::to_value(&q.jobs).map_err(|e| e.to_string())?,
    )?;
    // Presentation counters cannot authorize a transfer or redirect a path.
    // Failure to save them must not prevent a valid queue from transferring.
    let progress: HashMap<_, _> = q.jobs.iter().map(|job| (job.id, &job.progress)).collect();
    if let Err(error) = serde_json::to_value(progress)
        .map_err(|e| e.to_string())
        .and_then(|value| crate::storage::write_json("transfer-progress.json", &value))
    {
        tracing::warn!(%error, "transfer progress could not be saved; queue remains usable");
    }
    q.dirty = false;
    Ok(())
}
pub fn enqueue(
    session: &SshSession,
    upload: bool,
    local: String,
    remote: String,
    directory: bool,
    replace: bool,
) -> Result<(), String> {
    let job = Job {
        id: Uuid::new_v4(),
        account: crate::working_copies::identity(session),
        host: format!(
            "{}:{}",
            session.host_identity().host,
            session.host_identity().port
        ),
        upload,
        local,
        remote,
        directory,
        replace,
        state: State::Queued,
        error: None,
        progress: ProgressSnapshot::default(),
    };
    if !valid_job(&job) || session.is_closed() {
        return Err("Choose full local/server paths on a connected host".into());
    }
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    if !q.valid {
        return Err(q
            .error
            .clone()
            .unwrap_or_else(|| "Queue is unavailable".into()));
    }
    if q.jobs.len() >= MAX_JOBS {
        return Err("Transfer queue is full. Remove completed entries first.".into());
    }
    q.jobs.push(job);
    q.dirty = true;
    if let Err(e) = save(&mut q) {
        q.jobs.pop();
        return Err(e);
    }
    Ok(())
}
pub fn show() {
    queue().lock().unwrap_or_else(|e| e.into_inner()).show = true;
}
pub fn error() -> Option<String> {
    queue()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .error
        .clone()
}
pub fn has_jobs(session: &SshSession) -> bool {
    let account = crate::working_copies::identity(session);
    queue()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .jobs
        .iter()
        .any(|job| job.account == account)
}
pub fn is_busy() -> bool {
    let q = queue().lock().unwrap_or_else(|e| e.into_inner());
    !q.active.is_empty()
        || q.jobs
            .iter()
            .any(|j| matches!(j.state, State::Queued | State::Preparing | State::Running))
}
pub fn pause_all() {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    for active in q.active.values() {
        active.progress.file.cancel();
    }
    for j in &mut q.jobs {
        if matches!(j.state, State::Queued | State::Preparing | State::Running) {
            j.state = State::Paused;
        }
    }
    q.dirty = true;
    if let Err(e) = save(&mut q) {
        q.error = Some(e);
    }
}
pub fn generation() -> u64 {
    queue().lock().unwrap_or_else(|e| e.into_inner()).changed
}
pub fn poll(
    runtime: &crate::connection::Connections,
    sessions: &[Arc<SshSession>],
    ctx: &egui::Context,
) {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    let mut finished = vec![];
    let snapshots: Vec<_> = q
        .active
        .iter()
        .filter(|(_, active)| !active.cleanup)
        .map(|(id, active)| (*id, active.progress.snapshot()))
        .collect();
    for (id, snapshot) in snapshots {
        if let Some(job) = q.jobs.iter_mut().find(|job| job.id == id) {
            job.progress = snapshot;
        }
    }
    for (&id, active) in &mut q.active {
        match active.reply.try_recv() {
            Ok(result) => finished.push((id, result, active.cleanup)),
            Err(mpsc::error::TryRecvError::Disconnected) => finished.push((
                id,
                Err("Transfer worker stopped; private stages were preserved".into()),
                active.cleanup,
            )),
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
    }
    let mut remove = vec![];
    for (id, result, cleanup) in finished {
        let final_progress = q.active.remove(&id).map(|a| a.progress.snapshot());
        if let Some(j) = q.jobs.iter_mut().find(|j| j.id == id) {
            if !cleanup {
                if let Some(progress) = final_progress {
                    j.progress = progress;
                }
            }
            match result {
                Ok(()) if cleanup => remove.push(id),
                Ok(()) => {
                    j.state = State::Complete;
                    j.error = None;
                }
                Err(e) => {
                    if !matches!(j.state, State::Paused | State::Cancelled) {
                        j.state = State::Failed;
                    }
                    j.error = Some(e);
                }
            }
        }
        q.changed = q.changed.wrapping_add(1);
        q.dirty = true;
    }
    q.jobs.retain(|j| !remove.contains(&j.id));
    let pending: Vec<_> = q
        .jobs
        .iter()
        .filter(|j| j.state == State::Queued && !q.active.contains_key(&j.id))
        .cloned()
        .collect();
    for job in pending {
        if q.active.len() >= CONCURRENCY {
            break;
        }
        let Some(session) = sessions
            .iter()
            .find(|s| !s.is_closed() && crate::working_copies::identity(s) == job.account)
            .cloned()
        else {
            continue;
        };
        let progress = Arc::new(Progress::new());
        let (sender, reply) = mpsc::channel(1);
        if let Some(j) = q.jobs.iter_mut().find(|j| j.id == job.id) {
            j.state = State::Preparing;
            j.error = None;
        }
        q.dirty = true;
        if let Err(e) = save(&mut q) {
            q.error = Some(e);
            if let Some(j) = q.jobs.iter_mut().find(|j| j.id == job.id) {
                j.state = State::Paused;
            }
            break;
        }
        q.active.insert(
            job.id,
            Active {
                progress: progress.clone(),
                reply,
                cleanup: false,
            },
        );
        let context = ctx.clone();
        runtime.spawn(async move {
            let result = run(job, session, progress).await;
            let _ = sender.send(result).await;
            context.request_repaint();
        });
    }
    // UI state follows progress, but never requeues an active paused worker.
    let running: Vec<_> = q
        .active
        .iter()
        .filter(|(_, a)| a.progress.planned.load(Ordering::Acquire) && !a.cleanup)
        .map(|(id, _)| *id)
        .collect();
    let mut changed = false;
    for j in &mut q.jobs {
        if running.contains(&j.id) && j.state == State::Preparing {
            j.state = State::Running;
            changed = true;
        }
    }
    q.dirty |= changed;
    if let Err(e) = save(&mut q) {
        q.error = Some(e);
    }
    if !q.active.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
}
pub fn ui(ui: &mut egui::Ui, session: &Arc<SshSession>, runtime: &crate::connection::Connections) {
    rows(
        ui,
        Some(crate::working_copies::identity(session)),
        std::slice::from_ref(session),
        runtime,
    );
}
pub fn window(
    ctx: &egui::Context,
    sessions: &[Arc<SshSession>],
    runtime: &crate::connection::Connections,
) {
    let mut open = queue().lock().unwrap_or_else(|e| e.into_inner()).show;
    if !open {
        return;
    }
    egui::Window::new("File transfers")
        .open(&mut open)
        .default_size([680.0, 360.0])
        .show(ctx, |ui| {
            rows(ui, None, sessions, runtime);
        });
    queue().lock().unwrap_or_else(|e| e.into_inner()).show = open;
}
fn transfer_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let height = ui
        .text_style_height(&egui::TextStyle::Button)
        .max(ui.spacing().interact_size.y)
        + 2.0 * ui.spacing().button_padding.y;
    let width = [
        "Pause",
        "Resume / retry",
        "Cancel",
        "Remove entry",
        "Discard partial files",
    ]
    .into_iter()
    .map(|label| {
        egui::WidgetText::from(label)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Button,
            )
            .size()
            .x
    })
    .fold(0.0_f32, f32::max)
        + 2.0 * ui.spacing().button_padding.x;
    ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .min_size(egui::vec2(width, height))
            .wrap_mode(egui::TextWrapMode::Extend),
    )
}
fn rows(
    ui: &mut egui::Ui,
    account: Option<String>,
    sessions: &[Arc<SshSession>],
    runtime: &crate::connection::Connections,
) {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = &q.error {
        ui.label(e);
    }
    let mut actions = vec![];
    egui::ScrollArea::vertical()
        .id_salt(("transfer-queue", account.clone()))
        .max_height(ui.available_height().clamp(1.0, 240.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            for job in q.jobs.iter().filter(|j| account.as_ref().is_none_or(|a| a == &j.account)) {
                let active = q.active.get(&job.id);
                let connected = sessions.iter().any(|s| {
                    !s.is_closed() && crate::working_copies::identity(s) == job.account
                });
                ui.push_id(job.id, |ui| {
                    let progress = active
                        .filter(|a| !a.cleanup)
                        .map(|a| a.progress.snapshot())
                        .unwrap_or_else(|| job.progress.clone());
                    progress_ui(ui, job, &progress, active.is_some_and(|a| a.cleanup));
                    ui.horizontal_wrapped(|ui| {
                        if matches!(job.state, State::Preparing | State::Running | State::Queued)
                            && transfer_button(ui, "Pause", true).clicked()
                        {
                            actions.push((job.id, "pause"));
                        }
                        if matches!(job.state, State::Paused | State::Failed)
                            && transfer_button(ui, "Resume / retry", active.is_none() && connected).clicked()
                        {
                            actions.push((job.id, "resume"));
                        }
                        if !matches!(job.state, State::Complete | State::Cancelled)
                            && transfer_button(ui, "Cancel", true).clicked()
                        {
                            actions.push((job.id, "cancel"));
                        }
                        if job.state == State::Complete && active.is_none()
                            && transfer_button(ui, "Remove entry", true).clicked()
                        {
                            actions.push((job.id, "remove"));
                        }
                        if matches!(job.state, State::Cancelled | State::Failed | State::Paused)
                            && transfer_button(ui, "Discard partial files", active.is_none() && connected)
                                .on_hover_text("Remove only this transfer's private stage files and queue record. Published files remain.").clicked()
                        {
                            actions.push((job.id, "discard"));
                        }
                    });
                    if let Some(error) = &job.error {
                        ui.add(egui::Label::new(error).wrap());
                    }
                    ui.separator();
                });
            }
        });
    q.dirty |= !actions.is_empty();
    for (id, action) in actions {
        if action == "remove" {
            q.jobs.retain(|j| j.id != id);
            let _ = std::fs::remove_file(plan_path(id).unwrap_or_default());
            let _ = std::fs::remove_file(plan_notes_path(id).unwrap_or_default());
            continue;
        }
        if let Some(a) = q.active.get(&id) {
            a.progress.file.cancel();
        }
        let Some(job) = q.jobs.iter_mut().find(|j| j.id == id) else {
            continue;
        };
        match action {
            "pause" => job.state = State::Paused,
            "cancel" => job.state = State::Cancelled,
            "resume" => {
                job.state = State::Queued;
                job.error = None;
            }
            "discard" => {
                let job = job.clone();
                let Some(session) = sessions
                    .iter()
                    .find(|s| !s.is_closed() && crate::working_copies::identity(s) == job.account)
                    .cloned()
                else {
                    continue;
                };
                let (sender, reply) = mpsc::channel(1);
                let progress = Arc::new(Progress::new());
                let ctx = ui.ctx().clone();
                q.active.insert(
                    id,
                    Active {
                        progress,
                        reply,
                        cleanup: true,
                    },
                );
                runtime.spawn(async move {
                    let result = async {
                        if let Some(plan) = read_plan(&job)? {
                            for file in plan.files {
                                session
                                    .discard_transfer_stage(&file)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            }
                            tokio::fs::remove_file(plan_path(job.id)?)
                                .await
                                .map_err(|e| e.to_string())?;
                            let _ = tokio::fs::remove_file(plan_notes_path(job.id)?).await;
                        }
                        Ok(())
                    }
                    .await;
                    let _ = sender.send(result).await;
                    ctx.request_repaint();
                });
            }
            _ => {}
        }
    }
    if let Err(e) = save(&mut q) {
        q.error = Some(e);
    }
}
pub fn file_area(
    available: egui::Rect,
    row_height: f32,
    gap: f32,
    transfers: bool,
) -> (egui::Rect, Option<egui::Rect>) {
    if !transfers {
        return (available, None);
    }
    let height = (row_height * 7.0 + gap * 6.0).min(available.height().max(0.0) * 0.45);
    let footer = egui::Rect::from_min_max(
        egui::pos2(available.left(), available.bottom() - height),
        available.max,
    );
    let files = egui::Rect::from_min_max(
        available.min,
        egui::pos2(available.right(), (footer.top() - gap).max(available.top())),
    );
    (files, Some(footer))
}
fn progress_fraction(state: State, progress: &ProgressSnapshot) -> f32 {
    if state == State::Complete {
        return 1.0;
    }
    match progress.total {
        Some(total) if total > 0 => (progress.done as f64 / total as f64).clamp(0.0, 1.0) as f32,
        _ => 0.0,
    }
}
fn byte_size(bytes: u64) -> String {
    let mut value = bytes as f64;
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}
fn progress_ui(
    ui: &mut egui::Ui,
    job: &Job,
    progress: &ProgressSnapshot,
    cleanup: bool,
) -> egui::Response {
    let name = Path::new(&job.local)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    ui.add(egui::Label::new(name.as_ref()).truncate())
        .on_hover_text(format!("{} ↔ {}", job.local, job.remote));
    let busy = matches!(job.state, State::Preparing | State::Running);
    let finishing = busy && progress.total.is_some_and(|total| progress.done >= total);
    let label = if cleanup {
        "Removing partial files".into()
    } else if finishing {
        "Verifying and finishing".into()
    } else if busy && progress.total.is_none() {
        "Scanning files".into()
    } else if job.state == State::Running {
        format!("{:.0}%", progress_fraction(job.state, progress) * 100.0)
    } else if job.state == State::Complete {
        "Completed · 100%".into()
    } else {
        job.state.label().into()
    };
    let bar = ui.add(
        egui::ProgressBar::new(progress_fraction(job.state, progress))
            .desired_width(ui.available_width())
            .text(label)
            .animate(cleanup || (busy && (progress.total.is_none() || finishing))),
    );
    let mut detail = format!(
        "{} · {}",
        if job.upload { "Upload" } else { "Download" },
        job.host
    );
    if let Some(total) = progress.total {
        detail.push_str(&format!(
            " · {} / {}",
            byte_size(progress.done.min(total)),
            byte_size(total)
        ));
        if job.directory {
            detail.push_str(&format!(
                " · {} / {} files",
                progress.files_done, progress.files_total
            ));
        }
    }
    ui.add(egui::Label::new(detail).wrap());
    if progress.skipped_links > 0 {
        ui.colored_label(ui.visuals().warn_fg_color,format!("Skipped {} symbolic links",progress.skipped_links))
            .on_hover_text("Ordinary files and folders are copied. Symbolic links are not followed or copied, including links to files outside the selected folder.");
    }
    bar
}
fn plan_path(id: Uuid) -> Result<PathBuf, String> {
    if id.is_nil() {
        return Err("Invalid transfer identity".into());
    }
    let root = crate::storage::root()?.join("transfer-plans");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(root.join(format!("{id}.json")))
}
fn write_plan(plan: &Plan) -> Result<(), String> {
    let bytes = serde_json::to_vec(plan).map_err(|e| e.to_string())?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("Transfer plan exceeds 32 MiB".into());
    }
    // Keep display-only warnings outside the plan format read by older builds.
    crate::working_copies::atomic(
        &plan_notes_path(plan.id)?,
        plan.skipped_links.to_string().as_bytes(),
    )?;
    crate::working_copies::atomic(&plan_path(plan.id)?, &bytes)
}
fn plan_notes_path(id: Uuid) -> Result<PathBuf, String> {
    Ok(plan_path(id)?.with_extension("skipped.json"))
}
fn read_plan(job: &Job) -> Result<Option<Plan>, String> {
    let path = plan_path(job.id)?;
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    regular(&meta)?;
    if meta.len() > 32 * 1024 * 1024 {
        return Err("Oversized transfer plan preserved".into());
    }
    let mut plan: Plan = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "Damaged transfer plan preserved")?;
    if plan.id != job.id
        || plan.account != job.account
        || plan.files.len() + plan.directories.len() > MAX_ENTRIES
        || plan.files.iter().enumerate().any(|(i, f)| {
            f.job_id != job.id
                || f.ordinal as usize != i
                || f.upload != job.upload
                || f.validate().is_err()
                || !Path::new(&f.local_path).starts_with(&plan.local_root)
                || !remote_within(&f.remote_path, &plan.remote_root)
        })
        || plan.directories.iter().any(|d| {
            !Path::new(&d.local).starts_with(&plan.local_root)
                || !remote_within(&d.remote, &plan.remote_root)
        })
    {
        return Err("Transfer plan does not belong to the queued operation".into());
    }
    let notes = plan_notes_path(job.id)?;
    if std::fs::symlink_metadata(&notes).is_ok_and(|m| regular(&m).is_ok() && m.len() <= 32) {
        if let Ok(bytes) = std::fs::read(notes) {
            plan.skipped_links = serde_json::from_slice::<u64>(&bytes)
                .ok()
                .filter(|n| *n <= MAX_ENTRIES as u64)
                .unwrap_or(0);
        }
    }
    Ok(Some(plan))
}
async fn run(job: Job, session: Arc<SshSession>, progress: Arc<Progress>) -> Result<(), String> {
    if crate::working_copies::identity(&session) != job.account {
        return Err("Transfer belongs to a different host/account/key".into());
    }
    check(&progress.file)?;
    let plan = match read_plan(&job)? {
        Some(p) => p,
        None => {
            let p = prepare(&job, &session, &progress.file).await?;
            write_plan(&p)?;
            p
        }
    };
    let total = plan
        .files
        .iter()
        .try_fold(0u64, |a, f| a.checked_add(f.source_size))
        .ok_or("Transfer size overflow")?;
    progress.total.store(total, Ordering::Relaxed);
    progress
        .files_total
        .store(plan.files.len() as u64, Ordering::Relaxed);
    progress
        .skipped_links
        .store(plan.skipped_links, Ordering::Relaxed);
    progress.planned.store(true, Ordering::Release);
    for directory in &plan.directories {
        check(&progress.file)?;
        if job.upload {
            let parent = directory
                .remote
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p })
                .ok_or("Invalid destination directory")?;
            let resolved = metadata(&session, "stat", parent, &progress.file)
                .await?
                .ok_or("Destination parent missing")?;
            if resolved.kind != "directory" || resolved.canonical_path != parent {
                return Err("Destination parent changed; folder creation refused".into());
            }
            let existing =
                metadata(&session, "statOptional", &directory.remote, &progress.file).await?;
            match existing {
                Some(m) if m.kind == "directory" && m.canonical_path == directory.remote => {}
                Some(_) => return Err("Destination directory changed or is a link".into()),
                None => {
                    request(&session, "mkdir", &directory.remote, &progress.file).await?;
                }
            }
        } else {
            let parent = Path::new(&directory.local)
                .parent()
                .ok_or("Invalid local directory")?;
            if tokio::fs::canonicalize(parent)
                .await
                .map_err(|e| e.to_string())?
                != parent
            {
                return Err("Local parent changed; folder creation refused".into());
            }
            match tokio::fs::symlink_metadata(&directory.local).await {
                Ok(m) => directory_meta(&m)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tokio::fs::create_dir(&directory.local)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Err(e) => return Err(e.to_string()),
            }
            if tokio::fs::canonicalize(&directory.local)
                .await
                .map_err(|e| e.to_string())?
                != PathBuf::from(&directory.local)
            {
                return Err("Local destination directory changed".into());
            }
        }
    }
    for spec in plan.files {
        check(&progress.file)?;
        let key = if job.upload {
            format!("remote:{}:{}", job.account, spec.remote_path)
        } else {
            format!("local:{}", spec.local_path.to_lowercase())
        };
        let lock = destination_lock(key);
        let _guard = tokio::select! {guard=lock.lock()=>guard,_=progress.file.cancelled()=>return Err("Transfer paused while waiting for another transfer".into())};
        if job.upload {
            crate::file_workspace::check_remote(&session, &spec.remote_path)?;
        } else {
            crate::file_workspace::check_local(Path::new(&spec.local_path))?;
        }
        session
            .transfer_file_resumable(spec.clone(), progress.file.clone())
            .await
            .map_err(|e| e.to_string())?;
        progress.finish_file(spec.source_size);
    }
    Ok(())
}
fn check(p: &FileProgress) -> Result<(), String> {
    if p.is_cancelled() {
        Err("Transfer paused; its checkpoint was retained".into())
    } else {
        Ok(())
    }
}
fn req(operation: &str, path: &str) -> RemoteFileRequest {
    RemoteFileRequest {
        operation: operation.into(),
        path: path.into(),
        local_path: None,
        destination: None,
        expected_revision: None,
        permissions: None,
        overwrite: false,
        recursive: false,
    }
}
async fn request(
    session: &SshSession,
    op: &str,
    path: &str,
    p: &Arc<FileProgress>,
) -> Result<ad_ssh::RemoteFileResult, String> {
    check(p)?;
    session
        .file_operation(req(op, path), p.clone())
        .await
        .map_err(|e| e.to_string())
}
async fn metadata(
    session: &SshSession,
    op: &str,
    path: &str,
    p: &Arc<FileProgress>,
) -> Result<Option<RemoteFileMetadata>, String> {
    Ok(request(session, op, path, p).await?.metadata)
}
fn remote_within(path: &str, root: &str) -> bool {
    path == root
        || (root == "/" && path.starts_with('/'))
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}
fn join(parent: &str, name: &str) -> Result<String, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err("Invalid server filename".into());
    }
    Ok(format!("{}/{}", parent.trim_end_matches('/'), name))
}
fn regular(m: &std::fs::Metadata) -> Result<(), String> {
    no_links(m)?;
    if !m.is_file() {
        return Err("Transfer requires a regular file".into());
    }
    Ok(())
}
fn directory_meta(m: &std::fs::Metadata) -> Result<(), String> {
    no_links(m)?;
    if !m.is_dir() {
        return Err("Transfer requires a directory".into());
    }
    Ok(())
}
fn no_links(m: &std::fs::Metadata) -> Result<(), String> {
    if m.file_type().is_symlink() {
        return Err("Transfers do not follow symbolic links".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err("Transfers do not follow junctions or reparse points".into());
        }
    }
    Ok(())
}
async fn hash_local(path: &Path, p: &FileProgress) -> Result<(u64, String), String> {
    check(p)?;
    use tokio::io::AsyncReadExt;
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| e.to_string())?;
    regular(&meta)?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| e.to_string())?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut size = 0u64;
    let mut bytes = vec![0; 65536];
    loop {
        check(p)?;
        let n = file.read(&mut bytes).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
        size = size.checked_add(n as u64).ok_or("File too large")?;
    }
    Ok((
        size,
        hash.finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}
async fn destination_local(
    path: &Path,
    replace: bool,
    p: &FileProgress,
) -> Result<Option<String>, String> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(m) => {
            regular(&m)?;
            if !replace {
                return Err(format!(
                    "Destination exists: {}. Enable replacement explicitly or choose a new path.",
                    path.display()
                ));
            }
            crate::file_workspace::check_local(path)?;
            Ok(Some(hash_local(path, p).await?.1))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
async fn destination_remote(
    session: &SshSession,
    path: &str,
    replace: bool,
    p: &Arc<FileProgress>,
) -> Result<Option<String>, String> {
    if let Some(m) = metadata(session, "statOptional", path, p).await? {
        if m.kind != "file" || m.canonical_path != path {
            return Err("Destination is not a regular server file".into());
        }
        if !replace {
            return Err(format!("Server destination exists: {path}. Enable replacement explicitly or choose a new path."));
        }
        crate::file_workspace::check_remote(session, path)?;
        metadata(session, "revision", path, p)
            .await?
            .and_then(|m| m.revision)
            .map(Some)
            .ok_or_else(|| "Server destination revision unavailable".into())
    } else {
        Ok(None)
    }
}
async fn prepare(job: &Job, session: &SshSession, p: &Arc<FileProgress>) -> Result<Plan, String> {
    let local_input = PathBuf::from(&job.local);
    let local_parent = tokio::fs::canonicalize(local_input.parent().ok_or("No local parent")?)
        .await
        .map_err(|e| e.to_string())?;
    directory_meta(
        &tokio::fs::symlink_metadata(&local_parent)
            .await
            .map_err(|e| e.to_string())?,
    )?;
    let local = local_parent.join(
        local_input
            .file_name()
            .ok_or("Cannot transfer a filesystem root")?,
    );
    let (remote_parent, name) = job.remote.rsplit_once('/').ok_or("No server parent")?;
    let parent = metadata(
        session,
        "stat",
        if remote_parent.is_empty() {
            "/"
        } else {
            remote_parent
        },
        p,
    )
    .await?
    .ok_or("Server parent missing")?;
    if parent.kind != "directory" {
        return Err("Choose a server directory".into());
    }
    let remote = join(&parent.canonical_path, name)?;
    let mut plan = Plan {
        id: job.id,
        account: job.account.clone(),
        local_root: local_parent.display().to_string(),
        remote_root: parent.canonical_path,
        files: vec![],
        directories: vec![],
        skipped_links: 0,
    };
    let mut stack = vec![(local, remote, 0usize)];
    let mut local_targets = HashSet::new();
    while let Some((local, remote, depth)) = stack.pop() {
        check(p)?;
        if depth > 128
            || plan.files.len() + plan.directories.len() + plan.skipped_links as usize
                >= MAX_ENTRIES
        {
            return Err("Folder transfer exceeds the depth/entry limit".into());
        }
        if !local_targets.insert(local.to_string_lossy().to_lowercase()) {
            return Err("Server filenames collide on Windows; transfer refused".into());
        }
        if job.upload {
            let m = tokio::fs::symlink_metadata(&local)
                .await
                .map_err(|e| e.to_string())?;
            no_links(&m)?;
            if m.is_dir() {
                if !job.directory {
                    return Err("Source became a directory".into());
                }
                if let Some(dest) = metadata(session, "statOptional", &remote, p).await? {
                    if dest.kind != "directory" || dest.canonical_path != remote {
                        return Err("Server destination is not the chosen directory".into());
                    }
                }
                plan.directories.push(Directory {
                    local: local.display().to_string(),
                    remote: remote.clone(),
                });
                let mut entries = tokio::fs::read_dir(&local)
                    .await
                    .map_err(|e| e.to_string())?;
                while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                    let name = entry
                        .file_name()
                        .to_str()
                        .ok_or("Non-Unicode filename")?
                        .to_owned();
                    stack.push((entry.path(), join(&remote, &name)?, depth + 1));
                    if stack.len() + plan.files.len() + plan.directories.len() > MAX_ENTRIES {
                        return Err("Too many folder entries".into());
                    }
                }
                continue;
            }
            regular(&m)?;
            let (size, revision) = hash_local(&local, p).await?;
            let destination = destination_remote(session, &remote, job.replace, p).await?;
            let local = tokio::fs::canonicalize(local)
                .await
                .map_err(|e| e.to_string())?;
            plan.files.push(ResumableTransferFile {
                upload: true,
                job_id: job.id,
                ordinal: plan.files.len() as u32,
                local_parent: local.parent().unwrap().display().to_string(),
                local_path: local.display().to_string(),
                remote_parent: remote
                    .rsplit_once('/')
                    .map(|(p, _)| if p.is_empty() { "/" } else { p })
                    .unwrap()
                    .into(),
                remote_path: remote,
                source_revision: revision,
                source_size: size,
                destination_revision: destination,
            });
        } else {
            let m = metadata(session, "stat", &remote, p)
                .await?
                .ok_or("Server source missing")?;
            if skip_download_link(&m, job.directory, depth)? {
                plan.skipped_links += 1;
                continue;
            }
            if m.canonical_path != remote {
                return Err("Server source resolves outside the chosen folder".into());
            }
            if m.kind == "directory" {
                if !job.directory {
                    return Err("Source became a directory".into());
                }
                match tokio::fs::symlink_metadata(&local).await {
                    Ok(m) => directory_meta(&m)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.to_string()),
                }
                plan.directories.push(Directory {
                    local: local.display().to_string(),
                    remote: remote.clone(),
                });
                let result = request(session, "list", &remote, p).await?;
                let entries = result.entries.ok_or("Server directory listing missing")?;
                for entry in entries {
                    // Identify even dangling links without resolving their targets.
                    if entry.kind == "symlink" {
                        plan.skipped_links += 1;
                        if plan.files.len()
                            + plan.directories.len()
                            + stack.len()
                            + plan.skipped_links as usize
                            > MAX_ENTRIES
                        {
                            return Err("Too many folder entries".into());
                        }
                        continue;
                    }
                    if entry.path.is_empty() || entry.kind == "unsupported-name" {
                        return Err("Folder contains a non-UTF-8 server filename; transfer refused to avoid an incomplete copy".into());
                    }
                    let name = ad_desktop_model::files::safe_local_name(&entry.name);
                    if name != entry.name {
                        return Err(format!(
                            "Server filename is incompatible with Windows: {}",
                            entry.name
                        ));
                    }
                    stack.push((local.join(&name), join(&remote, &entry.name)?, depth + 1));
                    if stack.len()
                        + plan.files.len()
                        + plan.directories.len()
                        + plan.skipped_links as usize
                        > MAX_ENTRIES
                    {
                        return Err("Too many folder entries".into());
                    }
                }
                continue;
            }
            if m.kind != "file" {
                return Err("Folder contains a link or special file; transfer refused".into());
            }
            let source = metadata(session, "revision", &remote, p)
                .await?
                .ok_or("Server file missing")?;
            let destination = destination_local(&local, job.replace, p).await?;
            plan.files.push(ResumableTransferFile {
                upload: false,
                job_id: job.id,
                ordinal: plan.files.len() as u32,
                local_parent: local.parent().unwrap().display().to_string(),
                local_path: local.display().to_string(),
                remote_parent: remote
                    .rsplit_once('/')
                    .map(|(p, _)| if p.is_empty() { "/" } else { p })
                    .unwrap()
                    .into(),
                remote_path: remote,
                source_revision: source.revision.ok_or("Server revision missing")?,
                source_size: source.size.ok_or("Server size missing")?,
                destination_revision: destination,
            });
        }
    }
    Ok(plan)
}
fn skip_download_link(
    source: &RemoteFileMetadata,
    directory: bool,
    depth: usize,
) -> Result<bool, String> {
    if source.kind == "symlink" {
        if directory && depth > 0 {
            return Ok(true);
        }
        return Err("The selected source is a symbolic link. Select its target explicitly to copy that file or folder.".into());
    }
    Ok(false)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_link_warnings_survive_resume_without_changing_old_plan_format() {
        let folder = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(folder.path(), || {
            let job = progress_job(false);
            let link = RemoteFileMetadata {
                path: "/test/folder/link".into(),
                canonical_path: "/external/pdk/file".into(),
                kind: "symlink".into(),
                size: None,
                modified: None,
                permissions: None,
                uid: None,
                gid: None,
                revision: None,
                atomic_replace: false,
            };
            assert!(skip_download_link(&link, true, 1).unwrap());
            assert!(skip_download_link(&link, true, 0).is_err());
            assert!(skip_download_link(&link, false, 0).is_err());
            let mut file = link.clone();
            file.kind = "file".into();
            assert!(!skip_download_link(&file, true, 1).unwrap());
            let plan = Plan {
                id: job.id,
                account: job.account.clone(),
                local_root: job.local.clone(),
                remote_root: job.remote.clone(),
                directories: vec![],
                files: vec![],
                skipped_links: 13,
            };
            write_plan(&plan).unwrap();
            let bytes = std::fs::read(plan_path(job.id).unwrap()).unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                json.get("skippedLinks").is_none(),
                "older clients reject unknown plan fields"
            );
            assert_eq!(read_plan(&job).unwrap().unwrap().skipped_links, 13);
            std::fs::remove_file(plan_notes_path(job.id).unwrap()).unwrap();
            assert_eq!(read_plan(&job).unwrap().unwrap().skipped_links, 0);
            std::fs::write(
                plan_notes_path(job.id).unwrap(),
                b"9999999999999999999999999999999999999999",
            )
            .unwrap();
            assert_eq!(read_plan(&job).unwrap().unwrap().skipped_links, 0);
            let mut wrong = job.clone();
            wrong.account = "b".repeat(64);
            assert!(read_plan(&wrong).is_err());
        });
    }
    #[test]
    fn transfer_actions_have_equal_sizes_and_align_when_wrapping() {
        for font in [13.0, 22.0] {
            for width in [300.0, 600.0] {
                let ctx = egui::Context::default();
                ctx.style_mut(|s| {
                    s.text_styles
                        .insert(egui::TextStyle::Button, egui::FontId::proportional(font));
                });
                let mut rects = vec![];
                for _ in 0..3 {
                    rects.clear();
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for label in
                                        ["Resume / retry", "Cancel", "Discard partial files"]
                                    {
                                        rects.push(transfer_button(ui, label, true).rect);
                                    }
                                });
                            });
                        },
                    );
                }
                for r in &rects {
                    assert!((r.width() - rects[0].width()).abs() < 0.1);
                    assert!((r.height() - rects[0].height()).abs() < 0.1);
                    assert!(r.left() >= 0.0 && r.right() <= width);
                }
                for pair in rects.windows(2) {
                    assert!(!pair[0].intersects(pair[1]));
                }
            }
        }
    }
    fn progress_job(upload: bool) -> Job {
        Job {
            id: Uuid::new_v4(),
            account: "a".repeat(64),
            host: "example.test:22".into(),
            upload,
            local: "C:/test/folder-with-a-long-name-for-layout-testing".into(),
            remote: "/test/folder".into(),
            directory: true,
            replace: false,
            state: State::Running,
            error: None,
            progress: ProgressSnapshot::default(),
        }
    }
    #[test]
    fn folder_progress_counts_files_and_bytes_without_double_counting_file_transitions() {
        let progress = Progress::new();
        assert_eq!(progress.snapshot().total, None);
        progress.total.store(100, Ordering::Relaxed);
        progress.files_total.store(2, Ordering::Relaxed);
        progress.planned.store(true, Ordering::Release);
        progress.file.transferred.store(40, Ordering::Relaxed);
        assert_eq!(progress.snapshot().done, 40);
        progress.finish_file(40);
        assert_eq!(progress.snapshot().done, 40);
        assert_eq!(progress.snapshot().files_done, 1);
        progress.file.transferred.store(30, Ordering::Relaxed);
        assert_eq!(progress.snapshot().done, 70);
        assert!((progress_fraction(State::Running, &progress.snapshot()) - 0.7).abs() < 0.001);
        progress.file.transferred.store(60, Ordering::Relaxed);
        progress.finish_file(60);
        assert_eq!(progress.snapshot().done, 100);
        assert_eq!(progress.snapshot().files_done, 2);
        assert_eq!(
            progress_fraction(State::Complete, &progress.snapshot()),
            1.0
        );
        assert_eq!(
            progress_fraction(State::Running, &ProgressSnapshot::default()),
            0.0
        );
        assert_eq!(
            progress_fraction(State::Complete, &ProgressSnapshot::default()),
            1.0
        );
        assert_eq!(byte_size(1024), "1.0 KiB");
    }
    #[test]
    fn transfer_progress_keeps_old_queues_readable_and_retains_paused_counts() {
        let mut job = progress_job(false);
        let mut old = serde_json::to_value(&job).unwrap();
        old.as_object_mut().unwrap().remove("progress");
        let restored: Job = serde_json::from_value(old).unwrap();
        assert!(restored.progress.total.is_none());
        job.state = State::Paused;
        job.progress = ProgressSnapshot {
            done: 75,
            total: Some(100),
            files_done: 1,
            files_total: 2,
            skipped_links: 0,
        };
        let folder = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(folder.path(), || {
            let serialized = serde_json::to_value(&job).unwrap();
            assert!(
                serialized.get("progress").is_none(),
                "older clients reject inline progress"
            );
            let mut jobs = vec![serde_json::from_value::<Job>(serialized).unwrap()];
            let mut q = Queue {
                jobs: vec![job.clone()],
                active: HashMap::new(),
                valid: true,
                error: None,
                show: false,
                changed: 0,
                dirty: true,
            };
            save(&mut q).unwrap();
            let main = crate::storage::read_json("transfer-queue.json")
                .unwrap()
                .unwrap();
            assert!(main[0].get("progress").is_none());
            let mut inline = main[0].clone();
            inline["progress"] = serde_json::to_value(&job.progress).unwrap();
            assert_eq!(
                serde_json::from_value::<Job>(inline).unwrap().progress.done,
                75,
                "intermediate Build 6 queues remain readable"
            );
            restore_progress(&mut jobs);
            let restored = &jobs[0];
            assert_eq!(restored.progress.done, 75);
            assert_eq!(progress_fraction(restored.state, &restored.progress), 0.75);
        });
    }
    #[test]
    fn transfer_bars_stay_visible_below_file_panes_when_resized() {
        for upload in [false, true] {
            for font in [13.0, 22.0] {
                let ctx = egui::Context::default();
                ctx.style_mut(|style| {
                    style
                        .text_styles
                        .insert(egui::TextStyle::Body, egui::FontId::proportional(font));
                });
                for (width, height) in [(300.0, 240.0), (480.0, 400.0), (960.0, 600.0)] {
                    for _ in 0..3 {
                        let _ = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, height),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    let available = ui.available_rect_before_wrap();
                                    let (files, footer) = file_area(
                                        available,
                                        font.max(ui.spacing().interact_size.y),
                                        ui.spacing().item_spacing.y,
                                        true,
                                    );
                                    let footer = footer.unwrap();
                                    assert!(files.bottom() <= footer.top());
                                    assert!(available.contains_rect(footer));
                                    ui.allocate_rect(files, egui::Sense::hover());
                                    let mut child = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(footer)
                                            .layout(egui::Layout::top_down(egui::Align::Min)),
                                    );
                                    child.set_clip_rect(footer);
                                    let bar = progress_ui(
                                        &mut child,
                                        &progress_job(upload),
                                        &ProgressSnapshot {
                                            done: 50,
                                            total: Some(100),
                                            files_done: 1,
                                            files_total: 2,
                                            skipped_links: 0,
                                        },
                                        false,
                                    );
                                    assert!(
                                        footer.expand(0.1).contains_rect(bar.rect),
                                        "Bar escaped footer: {:?} / {:?}",
                                        bar.rect,
                                        footer
                                    );
                                });
                            },
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn transfer_boundaries_are_component_boundaries() {
        assert!(remote_within("/work/file", "/work"));
        assert!(!remote_within("/workspace/file", "/work"));
        assert!(remote_within("/file", "/"));
        assert!(join("/work", "../escape").is_err());
    }
    #[test]
    fn changed_local_destination_requires_explicit_replacement() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"existing").unwrap();
        let progress = FileProgress::default();
        assert!(rt
            .block_on(destination_local(&file, false, &progress))
            .is_err());
        assert_eq!(
            rt.block_on(destination_local(&file, true, &progress))
                .unwrap(),
            Some(rt.block_on(hash_local(&file, &progress)).unwrap().1)
        );
        assert_eq!(std::fs::read(file).unwrap(), b"existing");
    }
    #[test]
    fn canceled_scans_do_not_read_source_files() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let p = FileProgress::default();
        p.cancel();
        assert!(rt.block_on(hash_local(Path::new("missing"), &p)).is_err());
        assert!(check(&p).is_err());
    }
    #[test]
    fn overlapping_transfers_share_a_publish_lock() {
        let key = Uuid::new_v4().to_string();
        let a = destination_lock(key.clone());
        let b = destination_lock(key);
        assert!(Arc::ptr_eq(&a, &b));
        let _guard = a.try_lock().unwrap();
        assert!(b.try_lock().is_err());
    }
}
