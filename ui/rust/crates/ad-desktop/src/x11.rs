//! One authenticated bundled display shared by this application's X11 tabs.
//! Each tab keeps its own SSH forwarding cookie and a lease on the process.
use ad_ssh::X11ForwardingConfig;
use rand::RngCore;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
};

struct X11Server {
    #[cfg(windows)]
    job: Option<crate::process_job::ProcessJob>,
    child: Child,
    _directory: tempfile::TempDir,
    pub config: X11ForwardingConfig,
}

struct SharedServer(parking_lot::Mutex<X11Server>);

#[derive(Default)]
pub struct X11Runtime {
    // A weak reference ensures even an unexpectedly dropped last tab kills its
    // server. The gate serializes startup and final shutdown against reconnect.
    current: tokio::sync::Mutex<Weak<SharedServer>>,
    root: Option<PathBuf>,
}
pub struct X11Lease {
    runtime: Arc<X11Runtime>,
    server: Option<Arc<SharedServer>>,
    pub config: X11ForwardingConfig,
}
impl X11Runtime {
    pub async fn acquire(
        self: &Arc<Self>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<X11Lease, String> {
        let mut current = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("Connection cancelled".into()),
            current = self.current.lock() => current,
        };
        if cancel.is_cancelled() {
            return Err("Connection cancelled".into());
        }
        if let Some(server) = current.upgrade() {
            // A busy XGrabServer must not disconnect other tabs: inspect the
            // owned process only. Cookie probes are confined to initial startup.
            let running = server.0.lock().is_running()?;
            if running {
                return Ok(self.lease(server));
            }
        }
        let root = match &self.root {
            Some(root) => root.clone(),
            None => std::env::current_exe()
                .map_err(|e| e.to_string())?
                .parent()
                .ok_or("Application directory missing")?
                .join("x11"),
        };
        let server = X11Server::start(&root, cancel).await?;
        let server = Arc::new(SharedServer(parking_lot::Mutex::new(server)));
        *current = Arc::downgrade(&server);
        Ok(self.lease(server))
    }
    fn lease(self: &Arc<Self>, server: Arc<SharedServer>) -> X11Lease {
        let mut config = server.0.lock().config.clone();
        config.authentication_cookie = hexadecimal(&cookie());
        X11Lease {
            runtime: self.clone(),
            server: Some(server),
            config,
        }
    }
}
impl X11Lease {
    pub fn is_running(&self) -> Result<bool, String> {
        self.server
            .as_ref()
            .map(|server| server.0.lock().is_running())
            .unwrap_or(Ok(false))
    }
    pub async fn stop(&mut self) {
        let _gate = self.runtime.current.lock().await;
        if let Some(server) = self.server.take() {
            // Other tabs retain the same display. An old crashed generation can
            // only release its own Arc; it can never stop a replacement display.
            if let Ok(server) = Arc::try_unwrap(server) {
                server.0.into_inner().stop().await;
            }
        }
    }
}

fn preferences(directory: &Path) -> Result<String, String> {
    let directory = directory
        .to_str()
        .ok_or("The X11 runtime path is not valid Unicode")?;
    if directory.contains(['"', '\r', '\n', '\0']) {
        return Err("Invalid X11 runtime directory".into());
    }
    // VcXsrv accepts forward slashes in paths; avoid quoted backslash escapes.
    Ok(format!(
        "ICONDirectory \"{}\"\nDEFAULTICON \"aetherdesk.ico\"\n",
        directory.replace('\\', "/")
    ))
}

fn cookie() -> [u8; 16] {
    let mut bytes = [0; 16];
    for byte in &mut bytes {
        loop {
            *byte = rand::rngs::OsRng.next_u32() as u8;
            // VcXsrv's authority reader treats this byte as text-mode EOF.
            if *byte != 0x1a {
                break;
            }
        }
    }
    bytes
}
fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn packet(cookie: &[u8; 16]) -> Vec<u8> {
    let protocol = b"MIT-MAGIC-COOKIE-1";
    let mut packet = vec![b'l', 0, 11, 0, 0, 0, protocol.len() as u8, 0, 16, 0, 0, 0];
    packet.extend_from_slice(protocol);
    while packet.len() % 4 != 0 {
        packet.push(0);
    }
    packet.extend_from_slice(cookie);
    packet
}
async fn probe(port: u16, cookie: &[u8; 16], accepted: bool) -> bool {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await?;
        socket.write_all(&packet(cookie)).await?;
        let mut reply = [0; 8];
        socket.read_exact(&mut reply).await?;
        Ok::<bool, std::io::Error>(reply[0] == u8::from(accepted))
    })
    .await
    .is_ok_and(|result| result.unwrap_or(false))
}
fn child_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    command
}
impl X11Server {
    async fn start(
        root: &Path,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Self, String> {
        if cancel.is_cancelled() {
            return Err("Connection cancelled".into());
        }
        for name in [
            "vcxsrv.exe",
            "xauth.exe",
            "xkbcomp.exe",
            "fonts/misc/fonts.dir",
            "xkbdata/rules/base",
        ] {
            if !root.join(name).is_file() {
                return Err(format!("Bundled X11 file missing: {name}. Extract the complete portable package or reinstall."));
            }
        }
        let directory = tempfile::Builder::new()
            .prefix("aetherdesk-x11-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let authority = directory.path().join("Xauthority");
        tokio::fs::write(
            directory.path().join("aetherdesk.ico"),
            include_bytes!("../assets/theme/aetherdesk.ico"),
        )
        .await
        .map_err(|e| e.to_string())?;
        tokio::fs::write(
            directory.path().join(".XWinrc"),
            preferences(directory.path())?,
        )
        .await
        .map_err(|e| e.to_string())?;
        let local_cookie = cookie();
        let mut display = None;
        for number in 10..=39u16 {
            if cancel.is_cancelled() {
                return Err("Connection cancelled".into());
            }
            // Probe wildcard ownership as well as loopback, as the original does.
            if tokio::time::timeout(
                Duration::from_millis(250),
                TcpStream::connect(("127.0.0.1", 6000 + number)),
            )
            .await
            .is_ok_and(|result| result.is_ok())
            {
                continue;
            }
            if let Ok(reservation) = TcpListener::bind(("0.0.0.0", 6000 + number)).await {
                drop(reservation);
                display = Some(number);
                break;
            }
        }
        let display = display.ok_or("No free X11 display between :10 and :39")?;
        for host in ["127.0.0.1", "localhost"] {
            let mut command = child_command(&root.join("xauth.exe"));
            command
                .arg("-f")
                .arg(&authority)
                .arg("add")
                .arg(format!("{host}:{display}"))
                .arg(".")
                .arg(hexadecimal(&local_cookie));
            let status = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err("Connection cancelled".into()),
                status = tokio::time::timeout(Duration::from_secs(5), command.status()) =>
                    status.map_err(|_| "X11 authority setup timed out")?.map_err(|e| e.to_string())?,
            };
            if !status.success() {
                return Err("Could not initialize private X11 authentication".into());
            }
        }
        if cancel.is_cancelled() {
            return Err("Connection cancelled".into());
        }
        let mut child = child_command(&root.join("vcxsrv.exe"))
            .arg(format!(":{display}"))
            .args([
                "-multiwindow",
                "-notrayicon",
                "-noreset",
                "-clipboard",
                "-nowgl",
                "+iglx",
                "-auth",
            ])
            .arg(&authority)
            .arg("-logfile")
            .arg(directory.path().join("server.log"))
            .args(["-silent-dup-error", "-listen", "tcp", "-nolisten", "inet6"])
            .env("HOME", directory.path())
            .env("XKB_CONFIG_ROOT", root.join("xkbdata"))
            .env("XLOCALEDIR", root.join("locale"))
            .current_dir(root)
            .spawn()
            .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let job = match crate::process_job::ProcessJob::attach(&child) {
            Ok(job) => job,
            Err(_) => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                return Err("Could not attach the bundled X11 server to its cleanup job".into());
            }
        };
        let mut server = Self {
            #[cfg(windows)]
            job: Some(job),
            child,
            _directory: directory,
            config: X11ForwardingConfig {
                local_port: 6000 + display,
                authentication_protocol: "MIT-MAGIC-COOKIE-1".into(),
                authentication_cookie: hexadecimal(&cookie()),
                local_authentication_cookie: hexadecimal(&local_cookie),
                screen_number: 0,
            },
        };
        for _ in 0..40 {
            if cancel.is_cancelled() {
                server.stop().await;
                return Err("Connection cancelled".into());
            }
            if !server.is_running()? {
                return Err("Bundled X11 server exited during startup".into());
            }
            if probe(server.config.local_port, &local_cookie, true).await {
                if !probe(server.config.local_port, &[0x1a; 16], false).await {
                    server.stop().await;
                    return Err("X11 access control verification failed".into());
                }
                return Ok(server);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        server.stop().await;
        Err("The bundled X11 server did not become ready".into())
    }
    pub fn is_running(&mut self) -> Result<bool, String> {
        self.child
            .try_wait()
            .map(|s| s.is_none())
            .map_err(|e| e.to_string())
    }
    pub async fn stop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_packet_encodes_cookie_and_padding() {
        let bytes = packet(&[42; 16]);
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[12..30], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&bytes[32..], &[42; 16]);
    }
    #[test]
    fn cookie_avoids_windows_authority_eof() {
        for _ in 0..100 {
            let value = cookie();
            assert!(!value.contains(&0x1a));
            assert_eq!(hexadecimal(&value).len(), 32);
        }
    }
    #[test]
    fn preferences_use_private_icon_and_reject_directive_injection() {
        let text = preferences(Path::new(r"C:\Users\a user\private-x11")).unwrap();
        assert_eq!(
            text,
            "ICONDirectory \"C:/Users/a user/private-x11\"\nDEFAULTICON \"aetherdesk.ico\"\n"
        );
        assert!(preferences(Path::new("bad\"\nDEFAULTICON other")).is_err());
        let icon = include_bytes!("../assets/theme/aetherdesk.ico");
        assert_eq!(&icon[..4], &[0, 0, 1, 0]);
    }
    #[tokio::test]
    async fn waiting_tab_can_cancel_without_acquiring_or_stopping_a_display() {
        let runtime = Arc::new(X11Runtime::default());
        let gate = runtime.current.lock().await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let other = runtime.clone();
        let token = cancel.clone();
        let attempt = tokio::spawn(async move { other.acquire(&token).await });
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), attempt)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.err().as_deref(), Some("Connection cancelled"));
        assert!(gate.upgrade().is_none());
    }

    #[cfg(windows)]
    async fn fixed_font_works(config: &X11ForwardingConfig) -> bool {
        let mut local_cookie = [0; 16];
        for (index, byte) in local_cookie.iter_mut().enumerate() {
            *byte = u8::from_str_radix(
                &config.local_authentication_cookie[index * 2..index * 2 + 2],
                16,
            )
            .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut socket = TcpStream::connect(("127.0.0.1", config.local_port)).await?;
            socket.write_all(&packet(&local_cookie)).await?;
            let mut prefix = [0; 8];
            socket.read_exact(&mut prefix).await?;
            if prefix[0] != 1 {
                return Ok(false);
            }
            let mut setup = vec![0; usize::from(u16::from_le_bytes([prefix[6], prefix[7]])) * 4];
            socket.read_exact(&mut setup).await?;
            // ListFonts, maxNames=8, pattern="fixed", padded to four bytes.
            socket
                .write_all(&[
                    49, 0, 4, 0, 8, 0, 5, 0, b'f', b'i', b'x', b'e', b'd', 0, 0, 0,
                ])
                .await?;
            let mut reply = [0; 32];
            socket.read_exact(&mut reply).await?;
            Ok::<_, std::io::Error>(reply[0] == 1 && u16::from_le_bytes([reply[8], reply[9]]) != 0)
        })
        .await
        .is_ok_and(|result| result.unwrap_or(false))
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Run by packaging with AETHERDESK_X11_PACKAGE pointing to the completed package"]
    async fn bundled_runtime_sharing_authentication_icons_and_cleanup() {
        let package = PathBuf::from(
            std::env::var_os("AETHERDESK_X11_PACKAGE")
                .expect("Set AETHERDESK_X11_PACKAGE to the portable folder"),
        );
        let runtime = Arc::new(X11Runtime {
            root: Some(package.join("x11")),
            ..Default::default()
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        let (first, second) = tokio::join!(runtime.acquire(&cancel), runtime.acquire(&cancel));
        let mut first = first.unwrap();
        let mut second = second.unwrap();
        assert_eq!(first.config.local_port, second.config.local_port);
        assert_eq!(
            first.config.local_authentication_cookie,
            second.config.local_authentication_cookie
        );
        assert_ne!(
            first.config.authentication_cookie,
            second.config.authentication_cookie
        );
        assert_ne!(
            first.config.authentication_cookie,
            first.config.local_authentication_cookie
        );
        assert!(first.is_running().unwrap());
        let directory = first
            .server
            .as_ref()
            .unwrap()
            .0
            .lock()
            ._directory
            .path()
            .to_owned();
        assert!(directory.join("aetherdesk.ico").is_file());
        assert!(std::fs::read_to_string(directory.join(".XWinrc"))
            .unwrap()
            .contains("DEFAULTICON \"aetherdesk.ico\""));
        assert!(probe(first.config.local_port, &[0x1a; 16], false).await);
        assert!(fixed_font_works(&first.config).await);
        first.stop().await;
        first.stop().await; // Release is idempotent.
        assert!(second.is_running().unwrap());
        assert!(fixed_font_works(&second.config).await);
        second.stop().await;
        assert!(!directory.exists());

        let mut old = runtime.acquire(&cancel).await.unwrap();
        old.server
            .as_ref()
            .unwrap()
            .0
            .lock()
            .child
            .start_kill()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while old.is_running().unwrap() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let mut replacement = runtime.acquire(&cancel).await.unwrap();
        old.stop().await;
        assert!(replacement.is_running().unwrap());
        replacement.stop().await;
        assert!(runtime.current.lock().await.upgrade().is_none());
    }
}
