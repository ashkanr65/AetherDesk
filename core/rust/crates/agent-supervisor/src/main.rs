use std::ffi::OsString;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ad_stream_protocol::VideoCodec;
use agent_encoder::EncoderConfig;
use agent_input::InputConfig;
use agent_supervisor::{AgentServer, AgentServerConfig, SupervisorConfig};
use agent_wayland::compositor::{ApplicationSpec, CompositorConfig};
use tokio::io::AsyncWriteExt;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const ENDPOINT_PREFIX: &str = "AETHERDESK_AGENT_V1";
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
        Mode::Serve => serve(options).await?,
    }
    Ok(())
}

async fn launch(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let session_directory = session_directory(options.session_id)?;
    tokio::fs::create_dir_all(&session_directory).await?;
    tokio::fs::set_permissions(&session_directory, std::fs::Permissions::from_mode(0o700)).await?;
    let endpoint_path = session_directory.join("endpoint");
    if let Some(endpoint) = read_live_endpoint(&endpoint_path).await? {
        println!("{endpoint}");
        return Ok(());
    }

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
        .arg("--serve")
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
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn()?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(endpoint) = read_live_endpoint(&endpoint_path).await? {
            println!("AETHERDESK_AGENT_CREATED_V1");
            println!("{endpoint}");
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("agent startup timed out; inspect {}", log_path.display()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
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
    write_endpoint(
        &session_directory.join("endpoint"),
        server.endpoint().port,
        &server.endpoint().token,
    )
    .await?;
    server.run().await?;
    Ok(())
}

fn parse_options(mut arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mode = match arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .as_deref()
    {
        Some("--launch") => Mode::Launch,
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

async fn write_endpoint(path: &Path, port: u16, token: &[u8; 32]) -> Result<(), std::io::Error> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .await?;
    file.write_all(format!("{ENDPOINT_PREFIX} {port} {}\n", encode_hex(token)).as_bytes())
        .await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, path).await
}

async fn read_live_endpoint(path: &Path) -> Result<Option<String>, std::io::Error> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut fields = text.split_ascii_whitespace();
    if fields.next() != Some(ENDPOINT_PREFIX) {
        return Ok(None);
    }
    let Some(port) = fields.next().and_then(|value| value.parse::<u16>().ok()) else {
        return Ok(None);
    };
    let Some(token) = fields.next() else {
        return Ok(None);
    };
    if port == 0
        || token.len() != 64
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || fields.next().is_some()
    {
        return Ok(None);
    }
    if tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .is_err()
    {
        return Ok(None);
    }
    Ok(Some(format!("{ENDPOINT_PREFIX} {port} {token}")))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
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
    fn requires_a_session_identifier() {
        assert!(parse_options([OsString::from("--launch")].into_iter()).is_err());
    }

    #[test]
    fn rejects_shell_content_in_desktop_name() {
        assert!(validate_desktop("xfce;rm").is_err());
    }
}
