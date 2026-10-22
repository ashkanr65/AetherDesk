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
        atomic::{AtomicU64, Ordering},
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
}
#[derive(Clone, Serialize, Deserialize)]
struct Directory {
    local: String,
    remote: String,
}
struct Progress {
    file: Arc<FileProgress>,
    base: AtomicU64,
    total: AtomicU64,
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
        q.active.remove(&id);
        if let Some(j) = q.jobs.iter_mut().find(|j| j.id == id) {
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
        let progress = Arc::new(Progress {
            file: Arc::new(FileProgress::default()),
            base: AtomicU64::new(0),
            total: AtomicU64::new(0),
        });
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
        .filter(|(_, a)| a.progress.total.load(Ordering::Relaxed) > 0 && !a.cleanup)
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
    egui::ScrollArea::vertical().id_salt(("transfer-queue",account.clone())).max_height(170.0).show(ui,|ui|{
        for job in q.jobs.iter().filter(|j|account.as_ref().is_none_or(|a|a==&j.account)){
            let active=q.active.get(&job.id);
            ui.horizontal_wrapped(|ui|{
                ui.label(format!("{} · {} · {}",if job.upload{"Upload"}else{"Download"},job.state.label(),job.host));
                if let Some(active)=active{let done=active.progress.base.load(Ordering::Relaxed).saturating_add(active.progress.file.transferred.load(Ordering::Relaxed));let total=active.progress.total.load(Ordering::Relaxed);ui.label(format!("{done} / {total} bytes"));}
                if matches!(job.state,State::Preparing|State::Running|State::Queued)&&ui.small_button("Pause").clicked(){actions.push((job.id,"pause"));}
                let connected=sessions.iter().any(|s|!s.is_closed()&&crate::working_copies::identity(s)==job.account);
                if matches!(job.state,State::Paused|State::Failed)&&ui.add_enabled(active.is_none()&&connected,egui::Button::new("Resume / retry")).clicked(){actions.push((job.id,"resume"));}
                if !matches!(job.state,State::Complete|State::Cancelled)&&ui.small_button("Cancel").clicked(){actions.push((job.id,"cancel"));}
                if job.state==State::Complete&&active.is_none()&&ui.small_button("Remove entry").clicked(){actions.push((job.id,"remove"));}
                if matches!(job.state,State::Cancelled|State::Failed|State::Paused)&&ui.add_enabled(active.is_none()&&connected,egui::Button::new("Discard partial files")).on_hover_text("Remove only this transfer's private stage files and queue record. Published files remain.").clicked(){actions.push((job.id,"discard"));}
            });
            ui.add(egui::Label::new(format!("{} ↔ {}",job.local,job.remote)).selectable(true));
            if let Some(e)=&job.error{ui.label(e);}ui.separator();
        }
    });
    q.dirty |= !actions.is_empty();
    for (id, action) in actions {
        if action == "remove" {
            q.jobs.retain(|j| j.id != id);
            let _ = std::fs::remove_file(plan_path(id).unwrap_or_default());
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
                let progress = Arc::new(Progress {
                    file: Arc::new(FileProgress::default()),
                    base: AtomicU64::new(0),
                    total: AtomicU64::new(0),
                });
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
    crate::working_copies::atomic(&plan_path(plan.id)?, &bytes)
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
    let plan: Plan = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
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
    let mut complete = 0;
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
        progress.base.store(complete, Ordering::Relaxed);
        progress.file.transferred.store(0, Ordering::Relaxed);
        session
            .transfer_file_resumable(spec.clone(), progress.file.clone())
            .await
            .map_err(|e| e.to_string())?;
        complete += spec.source_size;
    }
    progress.base.store(total, Ordering::Relaxed);
    progress.file.transferred.store(0, Ordering::Relaxed);
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
    };
    let mut stack = vec![(local, remote, 0usize)];
    let mut local_targets = HashSet::new();
    while let Some((local, remote, depth)) = stack.pop() {
        check(p)?;
        if depth > 128 || plan.files.len() + plan.directories.len() >= MAX_ENTRIES {
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
                    if stack.len() + plan.files.len() + plan.directories.len() > MAX_ENTRIES {
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
#[cfg(test)]
mod tests {
    use super::*;
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
