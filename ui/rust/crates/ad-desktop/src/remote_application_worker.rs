//! Bounded request/response capture over this tab's authenticated SSH transport.
use ad_remote_session::{ssh_host::ApplicationHelper, AgentDeployment};
use ad_stream_protocol::window_stream::{self as wire, Reply, Request, WindowInfo};
use eframe::egui;
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
#[path = "application_frames.rs"]
mod frames;
pub(super) use frames::Frame;

#[derive(Clone)]
pub(crate) struct Target {
    pub ssh: Arc<ad_ssh::SshSession>,
    pub session: uuid::Uuid,
    pub deployment: AgentDeployment,
    pub runtime: tokio::runtime::Handle,
}
impl Target {
    pub(crate) async fn shell_environment(&self) -> Result<String, String> {
        let command = self.helper_command(ApplicationHelper::Environment)?;
        // The helper can precede the WM's atomic environment publication.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let output = self
                .ssh
                .execute_command_output(&command, Duration::from_secs(3))
                .await
                .map_err(|e| e.to_string())?;
            if output.exit_status == 0 {
                let value = String::from_utf8(output.stdout)
                    .map_err(|_| "Invalid application environment")?;
                let value = value.trim();
                if value.len() > 8192
                    || value.contains(['\n', '\r', '\0'])
                    || !value.starts_with(". '")
                {
                    return Err("Invalid application shell environment".into());
                }
                return Ok(format!("{value}; "));
            }
            if matches!(output.exit_status, 126 | 127) {
                return Err(
                    "The deployed host agent could not run the application environment helper."
                        .into(),
                );
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(
                    "Application environment did not become ready. Check the host agent log."
                        .into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    fn helper_command(&self, helper: ApplicationHelper) -> Result<String, String> {
        self.deployment
            .application_helper_command(helper, self.session)
            .map_err(|error| error.to_string())
    }

    fn command(&self) -> Result<String, String> {
        self.helper_command(ApplicationHelper::WindowMirror)
    }
}

#[derive(Clone)]
pub(super) struct PopupState {
    pub info: wire::Popup,
    pub image: Arc<egui::ColorImage>,
}
#[derive(Clone, Default)]
pub(super) struct State {
    pub input_epoch: u64,
    pub windows: Vec<WindowInfo>,
    pub selected: Option<WindowInfo>,
    pub image: Option<Frame>,
    pub popups: Vec<PopupState>,
    pub error: Option<String>,
    pub input_diagnostic: Option<(String, Instant)>,
    pub alive: bool,
    pub streaming: bool,
    pub host_titlebar: bool,
    pub titlebar: Option<PopupState>,
}
pub(super) struct Worker {
    pub commands: mpsc::Sender<Request>,
    pub state: Arc<Mutex<State>>,
    pub cancel: CancellationToken,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl Worker {
    pub fn start(target: Target, ctx: egui::Context) -> Self {
        let (commands, receive) = mpsc::channel(128);
        let state = Arc::new(Mutex::new(State {
            alive: true,
            ..Default::default()
        }));
        let cancel = CancellationToken::new();
        let s = state.clone();
        let c = cancel.clone();
        let runtime = target.runtime.clone();
        runtime.spawn(async move{
            let result=tokio::select!{_=c.cancelled()=>Ok(()),result=run(target,receive,s.clone(),ctx.clone())=>result};
            let mut state=s.lock();state.alive=false;if let Err(error)=result{state.error=Some(error);}drop(state);ctx.request_repaint();
        });
        Self {
            commands,
            state,
            cancel,
        }
    }
}
async fn write<S: AsyncWrite + Unpin>(stream: &mut S, request: &Request) -> Result<(), String> {
    let bytes = wire::encode(request)?;
    stream
        .write_all(wire::MAGIC)
        .await
        .map_err(|_| "Application connection closed")?;
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "Application connection closed")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "Application connection closed")?;
    stream
        .flush()
        .await
        .map_err(|_| "Cannot flush application command".into())
}
async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Reply, String> {
    let mut magic = [0; 8];
    stream
        .read_exact(&mut magic)
        .await
        .map_err(|_| "The server needs the updated AetherDesk agent for remote applications")?;
    if &magic != wire::MAGIC {
        return Err("Unexpected application stream identity".into());
    }
    let size = stream
        .read_u32()
        .await
        .map_err(|_| "Incomplete application response")? as usize;
    if size > wire::MAX_PACKET {
        return Err("Application response exceeds the size limit".into());
    }
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "Incomplete application response")?;
    wire::decode(&bytes)
}
#[path = "application_stream.rs"]
mod stream;
async fn run(
    target: Target,
    commands: mpsc::Receiver<Request>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
) -> Result<(), String> {
    stream::run(target, commands, state, ctx).await
}

fn quiet_focus_error(error: &str) -> bool {
    error.starts_with("Application focus changed")
        || error == "Activate the application in the remote desktop before typing"
}

fn batch_commands(
    first: Request,
    commands: &mut mpsc::Receiver<Request>,
    pending: &mut Option<Request>,
) -> Request {
    if !wire::batchable(&first) {
        return first;
    }
    let mut batch = vec![first];
    while batch.len() < 64 {
        match commands.try_recv() {
            Ok(next) if wire::batchable(&next) => batch.push(next),
            Ok(next) => {
                *pending = Some(next);
                break;
            }
            Err(_) => break,
        }
    }
    if batch.len() == 1 {
        batch.pop().unwrap()
    } else {
        Request::InputBatch(
            batch
                .into_iter()
                .map(|r| wire::WindowInput::try_from(r).expect("only eligible input is collected"))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_legacy_focus_notices_are_silent() {
        assert!(quiet_focus_error(
            "Application focus changed; click its independent view to activate it again"
        ));
        assert!(quiet_focus_error(
            "Activate the application in the remote desktop before typing"
        ));
        assert!(!quiet_focus_error("Connection timed out"));
        assert!(!quiet_focus_error("The application window was closed"));
    }
    #[test]
    fn input_batching_preserves_modifiers_release_order_and_capture_boundaries() {
        let (tx, mut rx) = mpsc::channel(128);
        tx.try_send(Request::Key {
            evdev: 30,
            pressed: true,
        })
        .unwrap();
        tx.try_send(Request::Key {
            evdev: 30,
            pressed: false,
        })
        .unwrap();
        tx.try_send(Request::Release).unwrap();
        tx.try_send(Request::Select(42)).unwrap();
        tx.try_send(Request::Move { x: 5, y: 9 }).unwrap();
        let mut pending = None;
        let batch = batch_commands(
            Request::Key {
                evdev: 42,
                pressed: true,
            },
            &mut rx,
            &mut pending,
        );
        let Request::InputBatch(input) = batch else {
            panic!("queued input used individual network round trips")
        };
        assert_eq!(input.len(), 4);
        assert!(matches!(
            input[0],
            wire::WindowInput::Key {
                evdev: 42,
                pressed: true
            }
        ));
        assert!(matches!(input[3], wire::WindowInput::Release));
        assert!(matches!(pending, Some(Request::Select(42))));
        assert!(matches!(rx.try_recv(), Ok(Request::Move { x: 5, y: 9 })));
        assert!(!wire::batchable(&Request::InputBatch(input)));
        assert!(!wire::batchable(&Request::Launch("xterm".into())));
    }
    #[test]
    fn application_helper_command_is_session_bound_and_rejects_shell_fragments() {
        let session = uuid::Uuid::new_v4();
        let mut deployment = AgentDeployment {
            version: "0.1.3".into(),
            remote_path: format!(
                ".aetherdesk/agent/0.1.3/bundles/{}/ad-agent",
                "ab".repeat(32)
            ),
            sha256: [1; 32],
            uploaded: true,
            bundled_runtime: true,
        };
        let bundle = deployment.remote_path.strip_suffix("/ad-agent").unwrap();
        for (mode, flag) in [
            (ApplicationHelper::Environment, "--application-environment"),
            (ApplicationHelper::WindowMirror, "--window-mirror"),
        ] {
            let text = deployment
                .application_helper_command(mode, session)
                .unwrap();
            assert!(text.starts_with("exec env "));
            assert!(text.contains(&format!("LD_LIBRARY_PATH=\"$HOME\"/'{bundle}/libs'")));
            assert!(text.ends_with(&format!(
                "\"$HOME\"/'{bundle}/ad-agent' {flag} --session-id {session}"
            )));
            assert!(!text.contains(".aetherdesk/agent/0.1.3/ad-agent"));
        }
        // An explicitly deployed legacy path is still valid.
        deployment.remote_path = ".aetherdesk/agent/0.1.3/ad-agent".into();
        assert!(deployment
            .application_helper_command(ApplicationHelper::Environment, session)
            .unwrap()
            .contains(&deployment.remote_path));
        for bad in [
            "../other",
            "evil;touch/ad-agent",
            "a b/ad-agent",
            "$(id)/ad-agent",
            "",
            "..",
        ] {
            deployment.remote_path = bad.into();
            assert!(deployment
                .application_helper_command(ApplicationHelper::WindowMirror, session)
                .is_err());
        }
    }
    #[tokio::test]
    async fn application_transport_rejects_oversized_or_wrong_identity_packets() {
        for wrong_magic in [true, false] {
            let (mut sender, mut receiver) = tokio::io::duplex(32);
            sender
                .write_all(if wrong_magic {
                    b"WRONG001"
                } else {
                    wire::MAGIC
                })
                .await
                .unwrap();
            sender.write_u32(wire::MAX_PACKET as u32 + 1).await.unwrap();
            assert!(read(&mut receiver).await.is_err());
        }
    }
}
