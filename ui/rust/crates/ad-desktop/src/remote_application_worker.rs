//! Bounded request/response capture over this tab's authenticated SSH transport.
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

#[derive(Clone)]
pub(crate) struct Target {
    pub ssh: Arc<ad_ssh::SshSession>,
    pub session: uuid::Uuid,
    pub version: String,
    pub runtime: tokio::runtime::Handle,
}
impl Target {
    fn command(&self) -> Result<String, String> {
        command(&self.version, self.session)
    }
}
fn command(version: &str, session: uuid::Uuid) -> Result<String, String> {
    if version.is_empty()
        || version.len() > 64
        || version == "."
        || version == ".."
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        return Err("Invalid remote application runtime version".into());
    }
    let directory = format!("\"$HOME/.aetherdesk/agent/{version}\"");
    Ok(format!("exec env LD_LIBRARY_PATH={directory}/libs PATH={directory}/libs:\"$PATH\" {directory}/ad-agent --window-mirror --session-id {session}"))
}
#[derive(Clone)]
pub(super) struct PopupState {
    pub info: wire::Popup,
    pub image: Arc<egui::ColorImage>,
}
#[derive(Clone, Default)]
pub(super) struct State {
    pub windows: Vec<WindowInfo>,
    pub selected: Option<WindowInfo>,
    pub image: Option<(u64, Arc<egui::ColorImage>)>,
    pub popups: Vec<PopupState>,
    pub error: Option<String>,
    pub alive: bool,
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
async fn run(
    target: Target,
    mut commands: mpsc::Receiver<Request>,
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
) -> Result<(), String> {
    let channel = tokio::time::timeout(
        Duration::from_secs(10),
        target.ssh.open_exec_channel(&target.command()?),
    )
    .await
    .map_err(|_| "Application connection timed out")?
    .map_err(|e| e.to_string())?;
    let mut stream = channel.into_stream();
    let ready = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
        .await
        .map_err(|_| "Application helper did not start")??;
    match ready {
        Reply::Ready => {}
        Reply::Error(error) => return Err(error),
        _ => return Err("Application helper did not negotiate the expected version".into()),
    }
    let mut selected = None;
    let mut previous: Option<(u64, u32, u32, Vec<u8>)> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(50));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut list_due = Instant::now() - Duration::from_secs(2);
    loop {
        let request = tokio::select! {
            biased;
            command=commands.recv()=>match command{Some(c)=>c,None=>Request::Stop},
            _=ticker.tick()=>{
                if selected.is_some(){Request::Poll}
                else if list_due.elapsed()>Duration::from_secs(1){list_due=Instant::now();Request::List}
                else{continue;}
            }
        };
        let stop = matches!(request, Request::Stop);
        tokio::time::timeout(Duration::from_secs(10), write(&mut stream, &request))
            .await
            .map_err(|_| "Application command timed out")??;
        let reply = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
            .await
            .map_err(|_| "Application capture timed out")??;
        match reply {
            Reply::Ready => return Err("Unexpected repeated application handshake".into()),
            Reply::Windows(windows) => {
                if windows.len() > wire::MAX_WINDOWS || windows.iter().any(|w| w.title.len() > 2048)
                {
                    return Err("Invalid application list".into());
                }
                state.lock().windows = windows;
            }
            Reply::Selected(window) => {
                wire::dimensions(window.width, window.height)?;
                selected = Some(window.id);
                previous = None;
                let mut s = state.lock();
                s.selected = Some(window);
                s.image = None;
                s.popups.clear();
                s.error = None;
            }
            Reply::Frame {
                window,
                sequence,
                base,
                width,
                height,
                tiles,
                popups,
            } => {
                if selected != Some(window) {
                    return Err("Application frame belongs to a different window".into());
                }
                wire::validate_popups(&popups)?;
                let popup_pixels: u64 = popups
                    .iter()
                    .map(|p| u64::from(p.width) * u64::from(p.height))
                    .sum();
                if u64::from(width) * u64::from(height) + popup_pixels > wire::MAX_PIXELS as u64 {
                    return Err("Application and menus exceed the capture budget".into());
                }
                let old = state.lock().popups.clone();
                let mut popup_images = Vec::new();
                for popup in popups {
                    let image = if let Some(old) = old.iter().find(|p| p.info == popup) {
                        old.image.clone()
                    } else {
                        let bytes = wire::apply_frame(
                            None,
                            1,
                            None,
                            popup.width,
                            popup.height,
                            popup.tiles.clone(),
                        )?;
                        Arc::new(egui::ColorImage::new(
                            [popup.width as usize, popup.height as usize],
                            bytes
                                .chunks_exact(4)
                                .map(|p| {
                                    egui::Color32::from_rgba_premultiplied(p[2], p[1], p[0], p[3])
                                })
                                .collect(),
                        ))
                    };
                    popup_images.push(PopupState { info: popup, image });
                }
                let only_menus = tiles.is_empty()
                    && previous.as_ref().is_some_and(|(seq, w, h, _)| {
                        base == Some(*seq) && sequence > *seq && *w == width && *h == height
                    });
                if only_menus {
                    let (_, w, h, bytes) = previous.take().unwrap();
                    previous = Some((sequence, w, h, bytes));
                    let mut s = state.lock();
                    if let Some((seq, _)) = &mut s.image {
                        *seq = sequence;
                    }
                    s.popups = popup_images;
                    s.error = None;
                    drop(s);
                    ctx.request_repaint();
                    continue;
                }
                let pixels = wire::apply_frame(
                    previous
                        .as_ref()
                        .map(|(seq, w, h, bytes)| (*seq, *w, *h, bytes.as_slice())),
                    sequence,
                    base,
                    width,
                    height,
                    tiles,
                )?;
                let image = egui::ColorImage::new(
                    [width as usize, height as usize],
                    pixels
                        .chunks_exact(4)
                        .map(|p| egui::Color32::from_rgb(p[2], p[1], p[0]))
                        .collect(),
                );
                previous = Some((sequence, width, height, pixels));
                let mut s = state.lock();
                s.image = Some((sequence, Arc::new(image)));
                s.popups = popup_images;
                s.error = None;
                if let Some(window) = &mut s.selected {
                    window.width = width;
                    window.height = height;
                }
            }
            Reply::Error(error) => {
                let mut s = state.lock();
                s.error = Some(error.chars().take(240).collect());
                if matches!(request, Request::Select(_) | Request::Poll) {
                    selected = None;
                    previous = None;
                    s.selected = None;
                    s.image = None;
                    s.popups.clear();
                }
            }
            Reply::Ack => state.lock().error = None,
            Reply::Idle => {}
        }
        ctx.request_repaint();
        if stop {
            let _ = stream.shutdown().await;
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_helper_command_is_session_bound_and_rejects_shell_fragments() {
        let session = uuid::Uuid::nil();
        let text = command("0.1.2", session).unwrap();
        assert!(text.contains("--window-mirror --session-id 00000000-0000-0000-0000-000000000000"));
        for bad in ["../other", "\";touch x", "a b", "$(id)", "", ".."] {
            assert!(command(bad, session).is_err());
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
