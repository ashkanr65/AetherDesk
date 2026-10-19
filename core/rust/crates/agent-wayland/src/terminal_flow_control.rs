//! Keep Ctrl+S available to applications instead of freezing terminal output.
//! Only PTY inputs belonging to this authenticated desktop are eligible. This
//! does not change shell startup files, terminal preferences or keyboard events.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const CHECK_INTERVAL: Duration = Duration::from_millis(500);

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
    worker: Option<JoinHandle<()>>,
}

impl TerminalFlowControl {
    pub(crate) fn start(display: u32, authority: PathBuf) -> io::Result<Self> {
        let scope = DesktopScope::new(display, &authority);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("aetherdesk-terminal-flow".into())
            .spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    // Process inspection and terminal ioctls stay off the
                    // compositor/input/render thread. Catch newly opened PTYs
                    // and shells that restore IXON after a foreground program.
                    if let Err(error) = update_terminals(&scope, &stopping) {
                        tracing::warn!(%error, "desktop terminal flow-control policy unavailable");
                        break;
                    }
                    thread::park_timeout(CHECK_INTERVAL);
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
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

fn start_time(process: &Path) -> io::Result<String> {
    let stat = fs::read_to_string(process.join("stat"))?;
    // comm can contain spaces and parentheses. Field 22 is starttime; fields
    // following its final ')' begin at field 3, the process state.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_ascii_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid process identity"))
}

fn session_terminal(process: &Path, scope: &DesktopScope) -> io::Result<Option<File>> {
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
    Ok(Some(terminal))
}

fn update_terminals(scope: &DesktopScope, stop: &AtomicBool) -> io::Result<()> {
    let mut visited = HashSet::new();
    for process in fs::read_dir("/proc")? {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(process) = process else { continue };
        if !process
            .file_name()
            .as_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        // Processes can exit at every step and procfs may hide other processes.
        // Fail closed: no matching identity/environment/input means no changes.
        let Ok(Some(terminal)) = session_terminal(&process.path(), scope) else {
            continue;
        };
        let Ok(metadata) = terminal.metadata() else {
            continue;
        };
        if visited.insert((metadata.dev(), metadata.ino(), metadata.rdev())) {
            if let Err(error) = prevent_output_pausing(&terminal) {
                tracing::debug!(%error, "could not update a desktop terminal's flow control");
            }
        }
    }
    Ok(())
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
        TerminalChild(
            Command::new("/bin/sleep")
                .arg("30")
                .env("DISPLAY", format!(":{display}"))
                .env("XAUTHORITY", authority)
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
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
        let guard = TerminalFlowControl::start(60998, authority).unwrap();
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
}
