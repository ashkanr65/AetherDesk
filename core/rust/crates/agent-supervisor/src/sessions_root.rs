//! Protection for the per-account directory shared by all session launch locks.
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use tokio::fs;

const OWNER_WRITE_SEARCH: u32 = 0o300;
const SHARED_WRITE: u32 = 0o022;

pub async fn prepare(path: &Path) -> io::Result<()> {
    let base = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing AetherDesk directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(base)
        .await
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("Cannot access AetherDesk directory {}: {e}", base.display()),
            )
        })?;
    // New folders get exactly 0700. For an existing folder preserve read
    // and search access and remove only group/other write permission.
    match fs::DirBuilder::new().mode(0o700).create(path).await {
        Ok(()) => set_private_mode(path).await?,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            change_owned_mode(path, |mode| {
                if mode & OWNER_WRITE_SEARCH == OWNER_WRITE_SEARCH {
                    mode & !SHARED_WRITE
                } else {
                    mode
                }
            }).await?;
        }
        Err(e) => return Err(io::Error::new(e.kind(), format!("Cannot create sessions directory {}: {e}. Check that your account can write to {}.", path.display(), base.display()))),
    }
    validate(path).await
}

/// Set permissions on the exact directory inode, including under a restrictive umask.
pub async fn set_private_mode(path: &Path) -> io::Result<()> {
    change_owned_mode(path, |_| 0o700).await
}

async fn change_owned_mode(path: &Path, target: fn(u32) -> u32) -> io::Result<()> {
    // O_PATH + O_NOFOLLOW opens a link itself, which owned_directory_mode
    // rejects. Use the same directory inode for the owner check and chmod.
    let dir = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
        .open(path)
        .await
        .map_err(|e| inspection_error(path, e))?;
    let metadata = dir
        .metadata()
        .await
        .map_err(|e| inspection_error(path, e))?;
    let mode = owned_directory_mode(path, &metadata)?;
    let target = target(mode);
    if mode != target {
        let fd_path = format!("/proc/self/fd/{}", dir.as_raw_fd());
        fs::set_permissions(fd_path, std::fs::Permissions::from_mode(target))
            .await
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!(
                        "Cannot change directory {} from mode {mode:04o} to {target:04o}: {e}. Ask the host administrator to check access.",
                        path.display()
                    ),
                )
            })?;
    }
    Ok(())
}

fn inspection_error(path: &Path, e: io::Error) -> io::Error {
    io::Error::new(
        e.kind(),
        format!(
            "Cannot inspect sessions directory {}: {e}. Check that your account can traverse its parent directories.",
            path.display()
        ),
    )
}

fn owned_directory_mode(path: &Path, meta: &std::fs::Metadata) -> io::Result<u32> {
    let uid = unsafe { libc::geteuid() };
    let mode = meta.mode() & 0o7777;
    let display = path.display();
    let problem = if !meta.is_dir() {
        let kind = if meta.file_type().is_symlink() {
            "a symbolic link"
        } else {
            "not a directory"
        };
        format!("Sessions path {display} is {kind}. Ask the host administrator to preserve the existing data and provide a real directory here owned by UID {uid} with mode 0700.")
    } else if meta.uid() != uid {
        format!("Sessions directory {display} is owned by UID {} (mode {mode:04o}); this connection runs as UID {uid}. Ask the host administrator to verify that this folder belongs to your account and, if confirmed, set its owner to UID {uid}.", meta.uid())
    } else {
        return Ok(mode);
    };
    Err(io::Error::new(ErrorKind::PermissionDenied, problem))
}

pub(crate) async fn validate(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)
        .await
        .map_err(|e| inspection_error(path, e))?;
    let mode = owned_directory_mode(path, &meta)?;
    let writable_by = match mode & SHARED_WRITE {
        0o002 => Some("other users"),
        0o020 => Some("the group"),
        0o022 => Some("the group and other users"),
        _ => None,
    };
    let missing_owner_access = mode & OWNER_WRITE_SEARCH != OWNER_WRITE_SEARCH;
    let (reason, change) = match (writable_by, missing_owner_access) {
        (None, false) => return Ok(()),
        (Some(who), false) => (format!("is writable by {who}"), "go-w"),
        (None, true) => ("lacks owner write or search permission".to_string(), "u+wx"),
        (Some(who), true) => (
            format!("is writable by {who} and lacks owner write or search permission"),
            "u+wx,go-w",
        ),
    };
    Err(io::Error::new(
        ErrorKind::PermissionDenied,
        format!("Sessions directory {} has mode {mode:04o} and {reason}. In this account's SSH terminal run: chmod {change} -- \"$HOME/.aetherdesk/sessions\"", path.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_launches_create_one_private_sessions_root() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".aetherdesk/sessions");
        let (first, second) = tokio::join!(prepare(&path), prepare(&path));
        first.unwrap();
        second.unwrap();
        assert_eq!(
            fs::symlink_metadata(&path).await.unwrap().mode() & 0o7777,
            0o700
        );
    }

    #[tokio::test]
    async fn owned_sessions_root_is_repaired_without_changing_retained_session() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("sessions");
        prepare(&path).await.unwrap();
        let session = path.join("retained");
        fs::create_dir(&session).await.unwrap();
        let endpoint = session.join("endpoint");
        fs::write(&endpoint, b"retained").await.unwrap();
        let session_before = fs::metadata(&session).await.unwrap();
        let open_endpoint = fs::File::open(&endpoint).await.unwrap();
        let root_before = fs::metadata(&path).await.unwrap();
        for (mode, expected) in [
            (0o775, 0o755),
            (0o757, 0o755),
            (0o777, 0o755),
            (0o770, 0o750),
            (0o755, 0o755),
            (0o700, 0o700),
        ] {
            fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .await
                .unwrap();
            let (first, second) = tokio::join!(prepare(&path), prepare(&path));
            first.unwrap();
            second.unwrap();
            let after = fs::metadata(&path).await.unwrap();
            assert_eq!(after.mode() & 0o7777, expected);
            assert_eq!(after.ino(), root_before.ino());
        }
        let session_after = fs::metadata(&session).await.unwrap();
        assert_eq!(session_after.mode(), session_before.mode());
        assert_eq!(session_after.ino(), session_before.ino());
        assert_eq!(
            fs::metadata(&endpoint).await.unwrap().ino(),
            open_endpoint.metadata().await.unwrap().ino()
        );
        assert_eq!(fs::read(&endpoint).await.unwrap(), b"retained");
    }

    #[tokio::test]
    async fn missing_owner_access_still_has_an_actionable_error() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("sessions");
        prepare(&path).await.unwrap();
        for (mode, fix) in [(0o500, "u+wx"), (0o520, "u+wx,go-w")] {
            fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .await
                .unwrap();
            let message = prepare(&path).await.unwrap_err().to_string();
            assert!(message.contains(&format!("{} has mode {mode:04o}", path.display())));
            assert!(message.contains("lacks owner write or search permission"));
            assert!(message.contains(&format!("chmod {fix} -- \"$HOME/.aetherdesk/sessions\"")));
            assert_eq!(fs::metadata(&path).await.unwrap().mode() & 0o7777, mode);
        }
    }

    #[tokio::test]
    async fn a_link_or_regular_file_at_the_sessions_path_is_preserved() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("sessions");
        let target = home.path().join("target");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&target)
            .await
            .unwrap();
        fs::set_permissions(&target, std::fs::Permissions::from_mode(0o775))
            .await
            .unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(prepare(&path)
            .await
            .unwrap_err()
            .to_string()
            .contains("symbolic link"));
        assert_eq!(fs::read_link(&path).await.unwrap(), target);
        assert_eq!(fs::metadata(&target).await.unwrap().mode() & 0o7777, 0o775);
        fs::remove_file(&path).await.unwrap();
        fs::write(&path, b"preserve this file").await.unwrap();
        assert!(prepare(&path)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a directory"));
        assert_eq!(fs::read(&path).await.unwrap(), b"preserve this file");
    }
}
