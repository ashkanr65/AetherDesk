//! Keep Ctrl+S available to applications instead of freezing terminal output.
//! Only PTY inputs belonging to this authenticated desktop are eligible. This
//! does not change shell startup files, terminal preferences or keyboard events.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const CHECK_INTERVAL: Duration = Duration::from_millis(500);
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(2);
const FALLBACK_INTERVAL: Duration = Duration::from_secs(30);

struct DesktopScope {
    display: Vec<u8>,
    authority: Vec<u8>,
}

impl DesktopScope {
    fn new(display: u32, authority: &Path) -> Self {
        let mut authority_entry = b"XAUTHORITY=".to_vec();
        authority_entry.extend_from_slice(authority.as_os_str().as_bytes());
        Self {
            display: format!("DISPLAY=:{display}").into_bytes(),
            authority: authority_entry,
        }
    }

    fn contains(&self, environment: &[u8]) -> bool {
        let mut display = None;
        let mut authority = None;
        for entry in environment.split(|byte| *byte == 0) {
            if entry.starts_with(b"DISPLAY=") {
                if display.is_some() {
                    return false;
                }
                // GTK may spell the same single-screen X server as :N.0.
                display = Some(
                    entry == self.display.as_slice()
                        || entry.strip_suffix(b".0") == Some(self.display.as_slice()),
                );
            }
            if entry.starts_with(b"XAUTHORITY=") {
                if authority.is_some() {
                    return false;
                }
                authority = Some(entry == self.authority.as_slice());
            }
        }
        display == Some(true) && authority == Some(true)
    }
}

pub(crate) struct TerminalFlowControl {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TerminalFlowControl {
    pub(crate) fn start(display: u32, authority: PathBuf, active: bool) -> io::Result<Self> {
        let scope = DesktopScope::new(display, &authority);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let active = Arc::new(AtomicBool::new(active));
        let viewed = active.clone();
        let worker = thread::Builder::new()
            .name("aetherdesk-terminal-flow".into())
            .spawn(move || {
                let mut terminals = TerminalCache::default();
                while !stopping.load(Ordering::Acquire) {
                    if !viewed.load(Ordering::Acquire) {
                        // Retained applications keep running. A desktop with no
                        // viewers needs no terminal inspection or polling.
                        thread::park();
                        continue;
                    }
                    // Process inspection and terminal ioctls stay off the
                    // compositor/input/render thread. Catch newly opened PTYs
                    // and shells that restore IXON after a foreground program.
                    if let Err(error) = terminals.update(&scope, &stopping) {
                        tracing::warn!(%error, "desktop terminal flow-control policy unavailable");
                        break;
                    }
                    thread::park_timeout(CHECK_INTERVAL);
                }
            })?;
        Ok(Self {
            stop,
            active,
            worker: Some(worker),
        })
    }

    pub(crate) fn set_active(&self, active: bool) {
        if self.active.swap(active, Ordering::AcqRel) != active {
            if let Some(worker) = &self.worker {
                worker.thread().unpark();
            }
        }
    }
}

impl Drop for TerminalFlowControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn start_time(process: &Path) -> io::Result<u64> {
    let stat = fs::read_to_string(process.join("stat"))?;
    // comm can contain spaces and parentheses. Field 22 is starttime; fields
    // following its final ')' begin at field 3, the process state.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_ascii_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process identity"))
}

fn session_terminal(process: &Path, scope: &DesktopScope) -> io::Result<Option<(File, u64)>> {
    if fs::metadata(process)?.uid() != unsafe { libc::geteuid() } {
        return Ok(None);
    }
    let input = process.join("fd/0");
    let target = fs::read_link(&input)?;
    let Some(number) = target
        .to_str()
        .and_then(|path| path.strip_prefix("/dev/pts/"))
    else {
        return Ok(None);
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let identity = start_time(process)?;
    let mut environment = Vec::new();
    File::open(process.join("environ"))?
        .take(256 * 1024 + 1)
        .read_to_end(&mut environment)?;
    if environment.len() > 256 * 1024 || !scope.contains(&environment) {
        return Ok(None);
    }
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&input)?;
    let metadata = terminal.metadata()?;
    if !metadata.file_type().is_char_device()
        || metadata.uid() != unsafe { libc::geteuid() }
        || identity != start_time(process)?
        || target != fs::read_link(&input)?
    {
        return Ok(None);
    }
    Ok(Some((terminal, identity)))
}

type TerminalId = (u64, u64, u64);
type OwnedPtys = HashMap<Vec<u8>, TerminalId>;

fn owned_ptys() -> io::Result<OwnedPtys> {
    let mut result = HashMap::new();
    for entry in fs::read_dir("/dev/pts")?.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.as_bytes();
        if name.is_empty() || !name.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.file_type().is_char_device() && metadata.uid() == unsafe { libc::geteuid() } {
            result.insert(
                name.to_vec(),
                (metadata.dev(), metadata.ino(), metadata.rdev()),
            );
        }
    }
    Ok(result)
}

#[derive(Default)]
struct PtyChanges {
    names: HashSet<Vec<u8>>,
    overflow: bool,
}

struct PtyWatch(File);
impl PtyWatch {
    fn open() -> io::Result<Self> {
        // Only device lifecycle/ownership events, never terminal input bytes.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let mask = libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_ATTRIB
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        if unsafe { libc::inotify_add_watch(fd, c"/dev/pts".as_ptr(), mask) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file))
    }

    fn changes(&mut self) -> io::Result<PtyChanges> {
        let mut result = PtyChanges::default();
        let mut bytes = [0; 4096];
        // A busy shared devpts mount cannot monopolize this worker.
        for _ in 0..4 {
            let count = match self.0.read(&mut bytes) {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if count == 0 {
                break;
            }
            let mut at = 0;
            while at + 16 <= count {
                let mask = u32::from_ne_bytes(bytes[at + 4..at + 8].try_into().unwrap());
                let len = u32::from_ne_bytes(bytes[at + 12..at + 16].try_into().unwrap()) as usize;
                let end = at + 16 + len;
                if end > count {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                if mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    return Err(io::Error::other("terminal device watch was removed"));
                }
                result.overflow |= mask & libc::IN_Q_OVERFLOW != 0;
                let name = bytes[at + 16..end].split(|byte| *byte == 0).next().unwrap();
                if !name.is_empty() && name.iter().all(u8::is_ascii_digit) {
                    result.names.insert(name.to_vec());
                }
                at = end;
            }
        }
        Ok(result)
    }
}

#[derive(Default)]
struct TerminalCache {
    processes: Vec<PathBuf>,
    discover_at: Option<Instant>,
    retry_at: Option<Instant>,
    fallback_at: Option<Instant>,
    initialized: bool,
    pending: bool,
    watch: Option<PtyWatch>,
    ptys: OwnedPtys,
    #[cfg(test)]
    discovery_passes: usize,
}
impl TerminalCache {
    fn discovery_due(&mut self, now: Instant) -> io::Result<bool> {
        if !self.initialized {
            self.initialized = true;
            self.pending = true;
            self.watch = match PtyWatch::open() {
                Ok(watch) => Some(watch),
                Err(error) => {
                    tracing::warn!(%error, "terminal device notifications unavailable; using bounded fallback discovery");
                    None
                }
            };
            // Install the watch before taking the snapshot: no PTY can appear
            // in the gap between the snapshot and event registration.
            self.ptys = owned_ptys()?;
        } else if let Some(watch) = &mut self.watch {
            match watch.changes() {
                Ok(changes) if changes.overflow || !changes.names.is_empty() => {
                    let current = owned_ptys()?;
                    self.pending |= changes.overflow
                        || current != self.ptys
                        || changes
                            .names
                            .iter()
                            .any(|name| self.ptys.contains_key(name) || current.contains_key(name));
                    self.ptys = current;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "terminal device notifications stopped; using bounded fallback discovery");
                    self.watch = None;
                    self.fallback_at = None;
                }
            }
        }
        if self.watch.is_none() && self.fallback_at.is_none_or(|at| now >= at) {
            self.pending = true;
            self.ptys = owned_ptys()?;
            // Do not synchronize fallback scans across retained agents.
            self.fallback_at = Some(
                now + FALLBACK_INTERVAL + Duration::from_secs(u64::from(std::process::id() % 17)),
            );
        }
        let discovery = self.discover_at.is_none_or(|at| now >= at)
            && (self.pending || self.retry_at.is_some_and(|at| now >= at));
        if discovery {
            // One settled follow-up catches PTY creation before fork/exec has
            // published the shell's final environment. No endless idle scans.
            self.retry_at = self.pending.then_some(now + DISCOVERY_INTERVAL);
            self.pending = false;
            self.discover_at = Some(now + DISCOVERY_INTERVAL);
        }
        Ok(discovery)
    }

    fn update(&mut self, scope: &DesktopScope, stop: &AtomicBool) -> io::Result<()> {
        self.update_at(scope, stop, Instant::now())
    }

    fn update_at(
        &mut self,
        scope: &DesktopScope,
        stop: &AtomicBool,
        now: Instant,
    ) -> io::Result<()> {
        let discovery = self.discovery_due(now)?;
        let candidates = if discovery {
            #[cfg(test)]
            {
                self.discovery_passes += 1;
            }
            fs::read_dir("/proc")?
                .filter_map(Result::ok)
                .filter(|p| p.file_name().as_bytes().iter().all(u8::is_ascii_digit))
                .map(|p| p.path())
                .collect::<Vec<_>>()
        } else {
            std::mem::take(&mut self.processes)
        };
        self.processes.clear();
        let mut visited = HashSet::new();
        let mut anchors: HashMap<TerminalId, (u64, PathBuf)> = HashMap::new();
        for process in candidates {
            if stop.load(Ordering::Acquire) {
                break;
            }
            // Processes can exit at every step and procfs may hide other processes.
            // Fail closed: no matching identity/environment/input means no changes.
            // Revalidate identity, environment and fd even on a cached path: PID
            // reuse, exec or a redirected stdin must never affect another session.
            let Ok(Some((terminal, started))) = session_terminal(&process, scope) else {
                // Rediscover a replacement for an exited/redirected anchor,
                // not every transient foreground child of the same PTY.
                if !discovery {
                    self.pending = true;
                }
                continue;
            };
            let Ok(metadata) = terminal.metadata() else {
                continue;
            };
            let id = (metadata.dev(), metadata.ino(), metadata.rdev());
            let anchor = anchors
                .entry(id)
                .or_insert_with(|| (started, process.clone()));
            if started < anchor.0 {
                *anchor = (started, process);
            }
            if visited.insert(id) {
                if let Err(error) = prevent_output_pausing(&terminal) {
                    tracing::debug!(%error, "could not update a desktop terminal's flow control");
                }
            }
        }
        self.processes = anchors.into_values().map(|(_, process)| process).collect();
        Ok(())
    }
}

fn prevent_output_pausing(terminal: &File) -> io::Result<()> {
    let fd = terminal.as_raw_fd();
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: tcgetattr initializes the complete termios value on success. The
    // File pins the descriptor for all three calls; no input is read or flushed.
    let mut attributes = unsafe {
        if libc::tcgetattr(fd, attributes.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        attributes.assume_init()
    };
    if attributes.c_iflag & libc::IXON == 0 {
        return Ok(());
    }
    attributes.c_iflag &= !libc::IXON;
    unsafe {
        if libc::tcsetattr(fd, libc::TCSANOW, &attributes) != 0
            || libc::tcflow(fd, libc::TCOON) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::FromRawFd;
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    fn pty() -> (File, File) {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        for fd in [master, slave] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0);
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
                0
            );
        }
        unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
    }

    fn attributes(terminal: &File) -> libc::termios {
        let mut value = std::mem::MaybeUninit::uninit();
        assert_eq!(
            unsafe { libc::tcgetattr(terminal.as_raw_fd(), value.as_mut_ptr()) },
            0
        );
        unsafe { value.assume_init() }
    }

    fn set_attributes(terminal: &File, value: &libc::termios) {
        assert_eq!(
            unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, value) },
            0
        );
    }

    fn read_bytes(terminal: &mut File, count: usize) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut result = vec![0; count];
        let mut offset = 0;
        while offset < count {
            assert!(
                Instant::now() < deadline,
                "terminal data did not arrive: {offset}/{count}"
            );
            let mut descriptor = libc::pollfd {
                fd: terminal.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
            if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            assert!(ready >= 0);
            if ready == 0 {
                continue;
            }
            match terminal.read(&mut result[offset..]) {
                Ok(0) => panic!("terminal closed after {offset}/{count} bytes"),
                Ok(read) => offset += read,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => panic!("terminal read failed: {error}"),
            }
        }
        result
    }

    #[test]
    fn terminal_policy_resumes_output_and_preserves_shortcut_bytes_and_other_flags() {
        let (mut master, mut slave) = pty();
        let mut original = attributes(&slave);
        original.c_iflag |= libc::IXON | libc::IXOFF | libc::ICRNL;
        original.c_lflag &= !(libc::ICANON | libc::ECHO);
        original.c_cc[libc::VMIN] = 1;
        original.c_cc[libc::VTIME] = 0;
        set_attributes(&slave, &original);
        let original = attributes(&slave);
        assert_eq!(unsafe { libc::tcflow(slave.as_raw_fd(), libc::TCOOFF) }, 0);
        prevent_output_pausing(&slave).unwrap();
        let updated = attributes(&slave);
        assert_eq!(updated.c_iflag, original.c_iflag & !libc::IXON);
        assert_eq!(updated.c_oflag, original.c_oflag);
        assert_eq!(updated.c_cflag, original.c_cflag);
        assert_eq!(updated.c_lflag, original.c_lflag);
        assert_eq!(updated.c_cc, original.c_cc);
        assert_eq!(unsafe { libc::cfgetispeed(&updated) }, unsafe {
            libc::cfgetispeed(&original)
        });
        assert_eq!(unsafe { libc::cfgetospeed(&updated) }, unsafe {
            libc::cfgetospeed(&original)
        });
        slave.write_all(b"resumed").unwrap();
        assert_eq!(read_bytes(&mut master, 7), b"resumed");
        // Ctrl+S, Ctrl+Q, Ctrl+A, Ctrl+W, Alt+F and an arrow remain actual
        // application input; the policy does not eat or synthesize any keys.
        let shortcuts = b"\x13\x11\x01\x17\x1bf\x1b[D";
        master.write_all(shortcuts).unwrap();
        assert_eq!(read_bytes(&mut slave, shortcuts.len()), shortcuts);
    }

    struct TerminalChild(Child);
    impl Drop for TerminalChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn terminal_child(slave: &File, display: u32, authority: &Path) -> TerminalChild {
        let mut child = TerminalChild(
            Command::new("/bin/sleep")
                .arg("30")
                .env("DISPLAY", format!(":{display}"))
                .env("XAUTHORITY", authority)
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // spawn() returning does not guarantee that procfs already exposes the
        // child's post-exec environment and descriptors. Wait for this fixture
        // to be observable before asserting the cache's discovery behaviour.
        // Probe only: do not change the terminal policy or consume any input.
        let process = PathBuf::from(format!("/proc/{}", child.0.id()));
        let scope = DesktopScope::new(display, authority);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = session_terminal(&process, &scope);
            if matches!(observed, Ok(Some(_))) {
                return child;
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "terminal fixture exited before becoming observable"
            );
            assert!(
                Instant::now() < deadline,
                "terminal fixture did not become observable: {observed:?}"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn terminal_policy_is_scoped_to_its_desktop_and_stops_with_its_guard() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("desktop.Xauthority");
        let (_master, owned) = pty();
        let (_other_master, unrelated) = pty();
        let _owned_child = terminal_child(&owned, 60998, &authority);
        // Even the same display number is insufficient: authority must match.
        let _unrelated_child =
            terminal_child(&unrelated, 60998, &authority.with_extension("other"));
        for slave in [&owned, &unrelated] {
            let mut value = attributes(slave);
            value.c_iflag |= libc::IXON;
            set_attributes(slave, &value);
        }
        let guard = TerminalFlowControl::start(60998, authority, true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while attributes(&owned).c_iflag & libc::IXON != 0 {
            assert!(Instant::now() < deadline, "owned terminal was not updated");
            thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(attributes(&unrelated).c_iflag & libc::IXON, 0);
        // Shells/fullscreen terminal programs can restore saved termios on
        // returning to the prompt. The policy must cover that transition too.
        let mut restored = attributes(&owned);
        restored.c_iflag |= libc::IXON;
        set_attributes(&owned, &restored);
        let deadline = Instant::now() + Duration::from_secs(5);
        while attributes(&owned).c_iflag & libc::IXON != 0 {
            assert!(Instant::now() < deadline, "restored IXON was not cleared");
            thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(attributes(&unrelated).c_iflag & libc::IXON, 0);
        drop(guard);
        let mut value = attributes(&owned);
        value.c_iflag |= libc::IXON;
        set_attributes(&owned, &value);
        thread::sleep(CHECK_INTERVAL * 2);
        assert_ne!(
            attributes(&owned).c_iflag & libc::IXON,
            0,
            "worker survived its session guard"
        );
        assert_ne!(attributes(&unrelated).c_iflag & libc::IXON, 0);
    }

    #[test]
    fn terminal_discovery_is_cached_while_known_terminals_are_revalidated() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("cache.Xauthority");
        let scope = DesktopScope::new(60996, &authority);
        let (_master, owned) = pty();
        let mut child = terminal_child(&owned, 60996, &authority);
        let mut cache = TerminalCache::default();
        let stop = AtomicBool::new(false);
        // Advance the cache's clock explicitly: scheduler delays on a busy
        // build machine must not accidentally expire the discovery interval.
        let now = Instant::now();
        cache.update_at(&scope, &stop, now).unwrap();
        assert_eq!(cache.processes.len(), 1);
        let scheduled = cache.discover_at;
        let mut value = attributes(&owned);
        value.c_iflag |= libc::IXON;
        set_attributes(&owned, &value);
        cache
            .update_at(&scope, &stop, now + CHECK_INTERVAL)
            .unwrap();
        assert_eq!(
            cache.discover_at, scheduled,
            "known PTYs caused another full discovery scan"
        );
        assert_eq!(attributes(&owned).c_iflag & libc::IXON, 0);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let (_new_master, new_terminal) = pty();
        let _new_child = terminal_child(&new_terminal, 60996, &authority);
        let mut new_attributes = attributes(&new_terminal);
        new_attributes.c_iflag |= libc::IXON;
        set_attributes(&new_terminal, &new_attributes);
        let mut value = attributes(&owned);
        value.c_iflag |= libc::IXON;
        set_attributes(&owned, &value);
        cache
            .update_at(&scope, &stop, now + CHECK_INTERVAL * 2)
            .unwrap();
        assert!(cache.processes.is_empty());
        assert_ne!(
            attributes(&owned).c_iflag & libc::IXON,
            0,
            "cached process path outlived its identity"
        );
        assert_ne!(
            attributes(&new_terminal).c_iflag & libc::IXON,
            0,
            "a cached update unexpectedly discovered a new terminal"
        );
        cache
            .update_at(&scope, &stop, now + DISCOVERY_INTERVAL)
            .unwrap();
        assert_eq!(cache.processes.len(), 1);
        assert_eq!(
            attributes(&new_terminal).c_iflag & libc::IXON,
            0,
            "scheduled discovery missed a new terminal"
        );
        assert_ne!(
            attributes(&owned).c_iflag & libc::IXON,
            0,
            "rediscovery changed an exited process's terminal"
        );
    }

    #[test]
    fn terminal_scope_requires_exact_display_and_authority_environment_entries() {
        let scope = DesktopScope::new(7, Path::new("/private/desktop.Xauthority"));
        assert!(scope.contains(b"DISPLAY=:7\0XAUTHORITY=/private/desktop.Xauthority\0"));
        assert!(scope.contains(b"DISPLAY=:7.0\0XAUTHORITY=/private/desktop.Xauthority\0"));
        for environment in [
            b"DISPLAY=:70\0XAUTHORITY=/private/desktop.Xauthority\0".as_slice(),
            b"DISPLAY=:7\0XAUTHORITY=/private/desktop.Xauthority.other\0",
            b"OTHER_DISPLAY=:7\0XAUTHORITY=/private/desktop.Xauthority\0",
            b"DISPLAY=:7\0",
            b"DISPLAY=:7\0DISPLAY=:8\0XAUTHORITY=/private/desktop.Xauthority\0",
            b"DISPLAY=:7\0XAUTHORITY=/private/desktop.Xauthority\0XAUTHORITY=/other\0",
        ] {
            assert!(!scope.contains(environment));
        }
    }

    #[test]
    fn terminal_discovery_stays_idle_until_pty_lifecycle_changes() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("event.Xauthority");
        let scope = DesktopScope::new(60995, &authority);
        let (_master, slave) = pty();
        let _child = terminal_child(&slave, 60995, &authority);
        let mut cache = TerminalCache::default();
        let stop = AtomicBool::new(false);
        let now = Instant::now();
        cache.update_at(&scope, &stop, now).unwrap();
        assert!(
            cache.watch.is_some(),
            "native devpts notifications unavailable"
        );
        cache
            .update_at(&scope, &stop, now + DISCOVERY_INTERVAL)
            .unwrap();
        let settled = cache.discovery_passes;
        // Hundreds of elapsed discovery intervals must not cause server-wide
        // PID/fd inspection when device identities have not changed.
        for seconds in [5, 10, 30, 60, 600] {
            let mut restored = attributes(&slave);
            restored.c_iflag |= libc::IXON;
            set_attributes(&slave, &restored);
            cache
                .update_at(&scope, &stop, now + Duration::from_secs(seconds))
                .unwrap();
            assert_eq!(attributes(&slave).c_iflag & libc::IXON, 0);
        }
        assert_eq!(
            cache.discovery_passes, settled,
            "idle terminal policy rescanned /proc"
        );

        let (_new_master, new_slave) = pty();
        let _new_child = terminal_child(&new_slave, 60995, &authority);
        let mut flags = attributes(&new_slave);
        flags.c_iflag |= libc::IXON;
        set_attributes(&new_slave, &flags);
        cache
            .update_at(&scope, &stop, now + Duration::from_secs(602))
            .unwrap();
        assert_eq!(cache.discovery_passes, settled + 1);
        assert_eq!(
            attributes(&new_slave).c_iflag & libc::IXON,
            0,
            "PTY event missed the new terminal"
        );
    }

    #[test]
    fn terminal_policy_retains_one_anchor_per_pty_and_ignores_foreground_exit() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("anchor.Xauthority");
        let scope = DesktopScope::new(60994, &authority);
        let (_master, slave) = pty();
        let shell = terminal_child(&slave, 60994, &authority);
        thread::sleep(Duration::from_millis(20));
        let mut foreground = terminal_child(&slave, 60994, &authority);
        let mut cache = TerminalCache::default();
        let stop = AtomicBool::new(false);
        let now = Instant::now();
        cache.update_at(&scope, &stop, now).unwrap();
        assert_eq!(
            cache.processes,
            [PathBuf::from(format!("/proc/{}", shell.0.id()))]
        );
        cache
            .update_at(&scope, &stop, now + DISCOVERY_INTERVAL)
            .unwrap();
        let settled = cache.discovery_passes;
        foreground.0.kill().unwrap();
        foreground.0.wait().unwrap();
        cache
            .update_at(&scope, &stop, now + Duration::from_secs(5))
            .unwrap();
        assert_eq!(cache.processes.len(), 1);
        assert_eq!(
            cache.discovery_passes, settled,
            "foreground exit caused a global scan"
        );
    }

    #[test]
    fn terminal_discovery_retries_after_pty_creation_before_shell_startup() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("startup.Xauthority");
        let scope = DesktopScope::new(60991, &authority);
        let mut cache = TerminalCache::default();
        let stop = AtomicBool::new(false);
        let now = Instant::now();
        cache.update_at(&scope, &stop, now).unwrap();
        cache
            .update_at(&scope, &stop, now + DISCOVERY_INTERVAL)
            .unwrap();
        let (_master, slave) = pty();
        // The event can precede the terminal emulator's child fork/exec.
        cache
            .update_at(&scope, &stop, now + Duration::from_secs(4))
            .unwrap();
        assert!(cache.processes.is_empty());
        let _child = terminal_child(&slave, 60991, &authority);
        cache
            .update_at(&scope, &stop, now + Duration::from_secs(6))
            .unwrap();
        assert_eq!(cache.processes.len(), 1);
        assert_eq!(attributes(&slave).c_iflag & libc::IXON, 0);
    }

    #[test]
    fn terminal_discovery_notification_failure_uses_a_bounded_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("fallback.Xauthority");
        let scope = DesktopScope::new(60990, &authority);
        let (_master, slave) = pty();
        let _child = terminal_child(&slave, 60990, &authority);
        let mut cache = TerminalCache {
            initialized: true,
            ..TerminalCache::default()
        };
        let stop = AtomicBool::new(false);
        let now = Instant::now();
        cache.update_at(&scope, &stop, now).unwrap();
        let next = cache.fallback_at.unwrap();
        assert!(
            (FALLBACK_INTERVAL..=FALLBACK_INTERVAL + Duration::from_secs(16))
                .contains(&next.duration_since(now))
        );
        cache
            .update_at(&scope, &stop, now + DISCOVERY_INTERVAL)
            .unwrap();
        for seconds in 3..30 {
            cache
                .update_at(&scope, &stop, now + Duration::from_secs(seconds))
                .unwrap();
        }
        assert_eq!(
            cache.discovery_passes, 2,
            "fallback recreated the two-second scan loop"
        );
        cache.update_at(&scope, &stop, next).unwrap();
        assert_eq!(cache.discovery_passes, 3);
        assert_eq!(attributes(&slave).c_iflag & libc::IXON, 0);
    }

    #[test]
    fn terminal_guard_parks_without_viewers_and_resumes_without_touching_other_flags() {
        let directory = tempfile::tempdir().unwrap();
        let authority = directory.path().join("viewed.Xauthority");
        let (_master, slave) = pty();
        let _child = terminal_child(&slave, 60993, &authority);
        let mut original = attributes(&slave);
        original.c_iflag |= libc::IXON | libc::IXOFF | libc::ICRNL;
        set_attributes(&slave, &original);
        let guard = TerminalFlowControl::start(60993, authority, false).unwrap();
        thread::sleep(CHECK_INTERVAL * 2);
        assert_ne!(
            attributes(&slave).c_iflag & libc::IXON,
            0,
            "unviewed guard was active"
        );
        guard.set_active(true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while attributes(&slave).c_iflag & libc::IXON != 0 {
            assert!(
                Instant::now() < deadline,
                "resumed guard did not revalidate its PTY"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(attributes(&slave).c_iflag, original.c_iflag & !libc::IXON);
        guard.set_active(false);
        thread::sleep(CHECK_INTERVAL * 2);
        let mut restored = attributes(&slave);
        restored.c_iflag |= libc::IXON;
        set_attributes(&slave, &restored);
        thread::sleep(CHECK_INTERVAL * 2);
        assert_ne!(attributes(&slave).c_iflag & libc::IXON, 0);
        let before = Instant::now();
        drop(guard);
        assert!(
            before.elapsed() < Duration::from_secs(1),
            "parked guard did not stop promptly"
        );
    }
}
