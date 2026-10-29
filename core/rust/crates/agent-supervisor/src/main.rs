use std::ffi::OsString;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ad_stream_protocol::VideoCodec;
use agent_encoder::EncoderConfig;
use agent_input::InputConfig;
use agent_supervisor::endpoint::read_live_endpoint;
use agent_supervisor::{AgentServer, AgentServerConfig, SupervisorConfig};
use agent_wayland::compositor::{ApplicationSpec, CompositorConfig};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
struct Options {
    mode: Mode,
    session_id: Uuid,
    desktop: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
    codec: VideoCodec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Launch,
    Watch,
    Serve,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--window-mirror") {
        let args: Vec<_> = std::env::args().skip(2).collect();
        if args.len() != 2 || args[0] != "--session-id" {
            return Err("usage: ad-agent --window-mirror --session-id UUID".into());
        }
        let session = Uuid::parse_str(&args[1])?.to_string();
        agent_wayland::window_mirror::serve(&session)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--version")) {
        println!("ad-agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let options = parse_options(std::env::args_os().skip(1))?;
    match options.mode {
        Mode::Launch => launch(options).await?,
        Mode::Watch => watch(options).await?,
        Mode::Serve => serve(options).await?,
    }
    Ok(())
}

async fn launch(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let session_directory = session_directory(options.session_id)?;
    // Keep the lock outside the removable session directory. Concurrent tabs
    // must never start competing brokers or race a confirmed-failed cleanup.
    tokio::fs::create_dir_all(
        session_directory
            .parent()
            .ok_or("Missing sessions directory")?,
    )
    .await?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(session_directory.with_file_name(format!(".{}.launch-lock", options.session_id)))?;
    let lock_deadline = Instant::now() + Duration::from_secs(25);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < lock_deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => {
                return Err(format!("Could not acquire desktop launch lock: {error}").into())
            }
        }
    }
    tokio::fs::create_dir_all(&session_directory).await?;
    tokio::fs::set_permissions(&session_directory, std::fs::Permissions::from_mode(0o700)).await?;
    let endpoint_path = session_directory.join("endpoint");
    if let Some(endpoint) = read_live_endpoint(&endpoint_path).await? {
        println!("{endpoint}");
        return Ok(());
    }

    // A closed listener can leave old discovery files after an abrupt exit.
    // Do not let those files race the next broker's atomic ready publication.
    for name in ["endpoint", "health", "agent-exit"] {
        match tokio::fs::remove_file(session_directory.join(name)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    // Confirmed-failed legacy cleanup removes the old directory only after
    // stopping its compositor. Recreate it while still holding the launch lock.
    tokio::fs::create_dir_all(&session_directory).await?;
    tokio::fs::set_permissions(&session_directory, std::fs::Permissions::from_mode(0o700)).await?;

    let executable = std::env::current_exe()?;
    let log_path = session_directory.join("agent.log");
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)?;
    let stderr = stdout.try_clone()?;
    let mut command = Command::new(executable);
    command
        .arg("--watch")
        .arg("--session-id")
        .arg(options.session_id.to_string())
        .arg("--desktop")
        .arg(&options.desktop)
        .arg("--width")
        .arg(options.width.to_string())
        .arg("--height")
        .arg(options.height.to_string())
        .arg("--fps")
        .arg(options.fps.to_string())
        .arg("--bitrate-kbps")
        .arg(options.bitrate_kbps.to_string())
        .arg("--codec")
        .arg(codec_name(options.codec))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    // A separate session, rather than just a process group, survives the SSH
    // command's logout/HUP. Its small watcher also survives a broker crash.
    detach_from_login(&mut command);
    let mut child = command.spawn()?;

    let startup_timeout = std::env::var("AETHERDESK_STARTUP_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| (1..=600).contains(v))
        .unwrap_or(0);
    let deadline = Instant::now() + START_TIMEOUT + Duration::from_secs(startup_timeout + 2);
    loop {
        if let Some(endpoint) = read_live_endpoint(&endpoint_path).await? {
            println!("AETHERDESK_AGENT_CREATED_V1");
            println!("{endpoint}");
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "agent startup stopped ({status}); inspect {}",
                log_path.display()
            )
            .into());
        }
        if Instant::now() >= deadline {
            return Err(format!("agent startup timed out; inspect {}", log_path.display()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn detach_from_login(command: &mut Command) {
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

async fn serve(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();
    let session_directory = session_directory(options.session_id)?;
    let runtime_directory = session_directory.join("runtime");
    tokio::fs::create_dir_all(&runtime_directory).await?;
    tokio::fs::set_permissions(&runtime_directory, std::fs::Permissions::from_mode(0o700)).await?;
    std::env::set_var("XDG_RUNTIME_DIR", &runtime_directory);

    let mut compositor = CompositorConfig::from_environment(options.width, options.height)?;
    compositor.refresh_millihz = options.fps * 1000;
    let server = AgentServer::bind(AgentServerConfig {
        supervisor: SupervisorConfig {
            session_id: options.session_id,
            compositor,
            input: InputConfig::default(),
            encoder: EncoderConfig {
                codec: options.codec,
                width: options.width,
                height: options.height,
                framerate: options.fps,
                bitrate_kbps: options.bitrate_kbps,
                keyframe_interval: options.fps,
            },
            application: desktop_application(&options.desktop)?,
        },
        session_directory: session_directory.clone(),
    })
    .await?;
    // The broker publishes its endpoint only after the desktop has started.
    server.run().await?;
    Ok(())
}

async fn watch(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::process::ExitStatusExt;
    let executable = std::env::current_exe()?;
    let startup = std::env::var("AETHERDESK_STARTUP_COMMANDS")
        .ok()
        .is_some_and(|s| !s.trim().is_empty());
    let mut command = if startup {
        let mut command = tokio::process::Command::new("bash");
        command
            .args([
                "-c",
                include_str!("../../ad-ssh/src/session_startup.sh"),
                "aetherdesk-startup",
                "exec",
            ])
            .arg(&executable);
        command
    } else {
        tokio::process::Command::new(&executable)
    };
    command.args(std::env::args_os().skip(1).map(|value| {
        if value == "--watch" {
            OsString::from("--serve")
        } else {
            value
        }
    }));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = command.spawn()?;
    let pid = child.id().unwrap_or(0);
    let status = child.wait().await?;
    let directory = session_directory(options.session_id)?;
    // Orderly end-session removes this directory. Never recreate ended sessions.
    if directory.is_dir() {
        let record = format!(
            "AETHERDESK_AGENT_EXIT_V1 pid={pid} code={:?} signal={:?} core_dump={}\n",
            status.code(),
            status.signal(),
            status.core_dumped()
        );
        let recorded = agent_supervisor::endpoint::record_process_exit(&directory, &record).await;
        // A full or size-limited agent.log must not panic the watcher before
        // its independent exit record has been written.
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{}", record.trim_end());
        recorded?;
    }
    Ok(())
}

fn parse_options(mut arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mode = match arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .as_deref()
    {
        Some("--launch") => Mode::Launch,
        Some("--watch") => Mode::Watch,
        Some("--serve") => Mode::Serve,
        _ => return Err("usage: ad-agent (--launch|--serve) --session-id UUID [options]".into()),
    };
    let mut session_id = None;
    let mut desktop = "xfce".to_owned();
    let mut width = 1920;
    let mut height = 1080;
    let mut fps = 60;
    let mut bitrate_kbps = 12_000;
    let mut codec = VideoCodec::H264;
    while let Some(flag) = arguments.next().and_then(|value| value.into_string().ok()) {
        let value = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--session-id" => {
                session_id = Some(Uuid::parse_str(&value).map_err(|_| "invalid session id")?)
            }
            "--desktop" => desktop = validate_desktop(&value)?.to_owned(),
            "--width" => width = parse_range(&value, 320, 16_384, "width")?,
            "--height" => height = parse_range(&value, 200, 16_384, "height")?,
            "--fps" => fps = parse_range(&value, 1, 240, "fps")?,
            "--bitrate-kbps" => bitrate_kbps = parse_range(&value, 128, 200_000, "bitrate")?,
            "--codec" => {
                codec = match value.as_str() {
                    "h264" => VideoCodec::H264,
                    "av1" => VideoCodec::Av1,
                    _ => return Err("unsupported codec".into()),
                }
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok(Options {
        mode,
        session_id: session_id.ok_or("missing --session-id")?,
        desktop,
        width,
        height,
        fps,
        bitrate_kbps,
        codec,
    })
}

fn parse_range(value: &str, minimum: u32, maximum: u32, name: &str) -> Result<u32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| format!("invalid {name}"))?;
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{name} is out of range"))
    }
}

fn validate_desktop(value: &str) -> Result<&str, String> {
    match value.to_ascii_lowercase().as_str() {
        "xfce" | "kde" | "gnome" | "mate" | "cinnamon" | "lxqt" => Ok(value),
        _ => Err("unsupported desktop environment".into()),
    }
}

fn desktop_application(desktop: &str) -> Result<ApplicationSpec, String> {
    let program = match desktop.to_ascii_lowercase().as_str() {
        "xfce" => "startxfce4",
        "kde" => "startplasma-x11",
        "gnome" => "gnome-session",
        "mate" => "mate-session",
        "cinnamon" => "cinnamon-session",
        "lxqt" => "startlxqt",
        _ => return Err("unsupported desktop environment".into()),
    };
    // Host applications and their window manager must share the rootful display.
    let mut application = ApplicationSpec::new("env");
    application.arguments = vec![
        "-u".into(),
        "WAYLAND_DISPLAY".into(),
        "dbus-run-session".into(),
        "--".into(),
        program.into(),
    ];
    application.environment = vec![
        ("GDK_BACKEND".into(), "x11".into()),
        ("QT_QPA_PLATFORM".into(), "xcb".into()),
        ("XDG_SESSION_TYPE".into(), "x11".into()),
    ];
    Ok(application)
}

fn session_directory(session_id: Uuid) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or("HOME is unavailable")?;
    Ok(PathBuf::from(home)
        .join(".aetherdesk")
        .join("sessions")
        .join(session_id.to_string()))
}

fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Av1 => "av1",
        _ => "h264",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn watcher_has_a_separate_login_session_and_its_exit_signal_is_observable() {
        use std::os::unix::process::ExitStatusExt;
        let mut command = Command::new("/bin/sleep");
        command.arg("10");
        detach_from_login(&mut command);
        let mut child = command.spawn().unwrap();
        let pid = child.id() as i32;
        let session = unsafe { libc::getsid(pid) };
        let parent = unsafe { libc::getsid(0) };
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert_eq!(session, pid);
        assert_ne!(session, parent);
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn requires_a_session_identifier() {
        assert!(parse_options([OsString::from("--launch")].into_iter()).is_err());
    }

    #[test]
    fn rejects_shell_content_in_desktop_name() {
        assert!(validate_desktop("xfce;rm").is_err());
    }
}
