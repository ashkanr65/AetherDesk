//! Kernel-released leases keep independent X11 viewers responsive even when
//! their separate desktop stream has detached. No PID/age guesses or app stops.
use std::{
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

fn open(runtime: &Path) -> io::Result<File> {
    let owner = std::fs::symlink_metadata(runtime)?;
    if !owner.is_dir() || owner.uid() != unsafe { libc::getuid() } || owner.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Viewer runtime must be private and owned",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(runtime.join("capture-viewers.lock"))?;
    let owner = file.metadata()?;
    if !owner.is_file() || owner.uid() != unsafe { libc::getuid() } || owner.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Viewer lease must be a private regular file",
        ));
    }
    Ok(file)
}

pub(crate) struct ViewerLease {
    _file: File,
}
impl ViewerLease {
    pub fn acquire(runtime: &Path) -> io::Result<Self> {
        let file = open(runtime)?;
        // The probe takes only a nonblocking, immediately released exclusive
        // lock. Bound retries rather than blocking a mirror/input worker.
        for _ in 0..20 {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
                return Ok(Self { _file: file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Viewer demand probe is busy",
        ))
    }
}

pub(crate) struct ViewerProbe {
    file: File,
}
impl ViewerProbe {
    pub fn open(runtime: &Path) -> io::Result<Self> {
        Ok(Self {
            file: open(runtime)?,
        })
    }
    pub fn active(&self) -> bool {
        if unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
            false
        } else {
            // An unknown lock error must preserve interactive frame callbacks.
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_viewer_leases_outlive_other_views_and_release_without_pid_files() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let probe = ViewerProbe::open(runtime.path()).unwrap();
        assert!(!probe.active());
        let first = ViewerLease::acquire(runtime.path()).unwrap();
        let second = ViewerLease::acquire(runtime.path()).unwrap();
        assert!(probe.active());
        drop(first);
        assert!(probe.active());
        drop(second);
        assert!(!probe.active());
    }
    #[test]
    fn viewer_leases_reject_links_and_shared_runtime_directories() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let other = tempfile::NamedTempFile::new().unwrap();
        symlink(other.path(), runtime.path().join("capture-viewers.lock")).unwrap();
        assert!(ViewerLease::acquire(runtime.path()).is_err());
        std::fs::remove_file(runtime.path().join("capture-viewers.lock")).unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ViewerProbe::open(runtime.path()).is_err());
    }
}
