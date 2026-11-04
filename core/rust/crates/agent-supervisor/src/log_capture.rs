//! Bounded host diagnostics; inherited desktop stderr never grows without limit.
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt};
use uuid::Uuid;

const LIMIT: u64 = 4 * 1024 * 1024;
const HISTORY: usize = 2;
const TAIL: usize = 16 * 1024;
const ARCHIVE_MAGIC: &str = "AETHERDESK_DIAGNOSTIC_V1\n";

pub struct HostLog {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    limit: u64,
    tail: Vec<u8>,
}
impl HostLog {
    pub fn open(directory: &Path) -> io::Result<Self> {
        let meta = std::fs::symlink_metadata(directory)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Invalid private log directory",
            ));
        }
        let mut log = Self {
            path: directory.join("agent.log"),
            file: None,
            written: 0,
            limit: LIMIT,
            tail: Vec::new(),
        };
        // Upgrading an old session must not retain an unbounded legacy log as
        // a rotation. Preserve just its newest bounded tail, without following
        // links or touching any names outside this session's diagnostic set.
        for path in [&log.path, &log.generation(1), &log.generation(2)] {
            match std::fs::symlink_metadata(path) {
                Ok(meta)
                    if meta.is_file()
                        && !meta.file_type().is_symlink()
                        && meta.uid() == unsafe { libc::geteuid() }
                        && meta.len() > LIMIT =>
                {
                    let mut old = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(path)?;
                    old.seek(SeekFrom::End(-(LIMIT as i64)))?;
                    let mut tail = Vec::new();
                    Read::by_ref(&mut old).take(LIMIT).read_to_end(&mut tail)?;
                    old.set_len(0)?;
                    old.seek(SeekFrom::Start(0))?;
                    old.write_all(&tail)?;
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        log.reopen()?;
        if let Some(file) = &mut log.file {
            file.seek(SeekFrom::Start(
                log.written.saturating_sub((TAIL / 2) as u64),
            ))?;
            Read::by_ref(file)
                .take((TAIL / 2) as u64)
                .read_to_end(&mut log.tail)?;
        }
        Ok(log)
    }
    fn reopen(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)?;
        self.written = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }
    fn generation(&self, n: usize) -> PathBuf {
        self.path.with_file_name(format!("agent.log.{n}"))
    }
    fn rotate(&mut self) -> io::Result<()> {
        self.file.take();
        match std::fs::remove_file(self.generation(HISTORY)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        for n in (1..HISTORY).rev() {
            match std::fs::rename(self.generation(n), self.generation(n + 1)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        match std::fs::rename(&self.path, self.generation(1)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.reopen()
    }
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        let bytes = &bytes[bytes.len().saturating_sub(self.limit as usize)..];
        let newest = &bytes[bytes.len().saturating_sub(TAIL / 2)..];
        let discard = (self.tail.len() + newest.len()).saturating_sub(TAIL / 2);
        self.tail.drain(..discard);
        self.tail.extend_from_slice(newest);
        if self.file.is_none() {
            self.reopen()?;
        }
        if self.written.saturating_add(bytes.len() as u64) > self.limit {
            self.rotate()?;
        }
        let result = self.file.as_mut().expect("opened log").write_all(bytes);
        // A partial disk-full write must not corrupt the size accounting.
        self.written = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(self.limit);
        result
    }
    pub fn archive(&mut self, directory: &Path, session: Uuid, exit: &str) -> io::Result<()> {
        match std::fs::create_dir(directory) {
            Ok(()) => std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(directory)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Invalid diagnostic archive",
            ));
        }
        let path = directory.join(format!("{session}.log"));
        let mut archive = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        archive.write_all(ARCHIVE_MAGIC.as_bytes())?;
        archive.write_all(&self.tail)?;
        archive.write_all(b"\n")?;
        archive.write_all(&exit.as_bytes()[..exit.len().min(1024)])?;
        archive.flush()?;
        prune_archives(directory)
    }
}

fn prune_archives(directory: &Path) -> io::Result<()> {
    let mut files = Vec::new();
    for item in std::fs::read_dir(directory)?.take(4096) {
        let item = item?;
        let path = item.path();
        if path.extension().and_then(|s| s.to_str()) != Some("log")
            || path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .is_none()
        {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.len() > TAIL as u64
        {
            continue;
        }
        let mut prefix = vec![0; ARCHIVE_MAGIC.len()];
        if File::open(&path)?.read_exact(&mut prefix).is_err() || prefix != ARCHIVE_MAGIC.as_bytes()
        {
            continue;
        }
        files.push((meta.modified()?, path));
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (index, (modified, path)) in files.into_iter().enumerate() {
        if index >= 20
            || SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default()
                > Duration::from_secs(7 * 86400)
        {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn drain<R: AsyncRead + Unpin + Send + 'static>(
    mut reader: R,
    log: Arc<Mutex<HostLog>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = vec![0; 8192];
        loop {
            let count = match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let bytes = buffer[..count].to_vec();
            let log = log.clone();
            // Disk I/O belongs to this separate watcher, never input/render.
            // Keep draining on ENOSPC so stderr cannot wedge the desktop.
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(mut log) = log.lock() {
                    let _ = log.append(&bytes);
                }
            })
            .await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_logs_rotate_and_archive_a_bounded_tail_without_removing_unknown_files() {
        let root = tempfile::tempdir().unwrap();
        let mut log = HostLog::open(root.path()).unwrap();
        log.limit = 128;
        for _ in 0..20 {
            log.append(&[b'x'; 100]).unwrap();
        }
        for n in 0..=2 {
            let p = if n == 0 {
                log.path.clone()
            } else {
                log.generation(n)
            };
            assert!(p.metadata().unwrap().len() <= 128);
        }
        let archive = root.path().join("diagnostics");
        for _ in 0..23 {
            log.archive(&archive, Uuid::new_v4(), "exit=0").unwrap();
        }
        assert_eq!(std::fs::read_dir(&archive).unwrap().count(), 20);
        let unknown = archive.join(format!("{}.log", Uuid::new_v4()));
        std::fs::write(&unknown, b"user data, not an owned diagnostic").unwrap();
        for _ in 0..3 {
            log.archive(&archive, Uuid::new_v4(), "exit=101").unwrap();
        }
        assert!(unknown.exists());
        assert_eq!(std::fs::read_dir(&archive).unwrap().count(), 21);
    }
    #[test]
    fn legacy_host_logs_are_trimmed_to_their_newest_bytes_before_rotation() {
        let root = tempfile::tempdir().unwrap();
        for name in ["agent.log", "agent.log.1", "agent.log.2"] {
            let mut bytes = vec![b'x'; LIMIT as usize + 1024];
            bytes.extend_from_slice(b"newest fixture line");
            std::fs::write(root.path().join(name), &bytes).unwrap();
        }
        let _log = HostLog::open(root.path()).unwrap();
        for name in ["agent.log", "agent.log.1", "agent.log.2"] {
            let bytes = std::fs::read(root.path().join(name)).unwrap();
            assert_eq!(bytes.len(), LIMIT as usize);
            assert!(bytes.ends_with(b"newest fixture line"));
        }
    }
    #[test]
    fn ending_a_desktop_preserves_a_diagnostic_tail_without_recreating_its_runtime() {
        let root = tempfile::tempdir().unwrap();
        let session = root.path().join("session");
        std::fs::create_dir(&session).unwrap();
        let mut log = HostLog::open(&session).unwrap();
        log.append(b"startup failure fixture").unwrap();
        std::fs::remove_dir_all(&session).unwrap();
        // Rotation cannot reopen a deleted runtime; its memory tail must survive.
        log.limit = 1;
        assert!(log.append(b"worker stopped").is_err());
        let id = Uuid::new_v4();
        let archives = root.path().join("diagnostics");
        log.archive(&archives, id, "exit=101").unwrap();
        let saved = std::fs::read_to_string(archives.join(format!("{id}.log"))).unwrap();
        assert!(saved.contains("startup failure fixture"));
        assert!(saved.contains("exit=101"));
        assert!(!session.exists());
    }
    #[tokio::test]
    async fn failed_log_writes_do_not_stop_draining_the_worker() {
        let root = tempfile::tempdir().unwrap();
        let mut log = HostLog::open(root.path()).unwrap();
        log.file = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/full")
                .unwrap(),
        );
        let (mut writer, reader) = tokio::io::duplex(1024);
        let task = drain(reader, Arc::new(Mutex::new(log)));
        use tokio::io::AsyncWriteExt;
        tokio::time::timeout(Duration::from_secs(3), writer.write_all(&vec![b'x'; 64000]))
            .await
            .unwrap()
            .unwrap();
        drop(writer);
        task.await.unwrap();
    }
}
