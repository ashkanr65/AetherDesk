//! A local, revocable connector. The GUI owns shares and every action approval.
use crate::{
    ai_tools::{self, Workspace},
    codex_protocol as protocol,
    connection::Connections,
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    sync::{mpsc, oneshot, Semaphore},
};
use tokio_util::sync::CancellationToken;

pub struct Target {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub workspace: Workspace,
}
struct Call {
    name: String,
    args: Value,
    epoch: u64,
    cancel: CancellationToken,
    reply: oneshot::Sender<Value>,
    created: Instant,
}
struct Server {
    cancel: CancellationToken,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Running {
    cancel: CancellationToken,
    finished: CancellationToken,
}
struct Descriptor {
    root: PathBuf,
    instance: String,
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        let path = self.root.join(protocol::RUNTIME_FILE);
        if std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|data| data["instance"] == self.instance)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}
fn authenticate(envelope: &Value, instance: &str, key: &ring::hmac::Key) -> Result<Value, String> {
    use base64::Engine;
    let payload = &envelope["payload"];
    let bytes = serde_json::to_vec(payload).map_err(|_| "Invalid connector envelope")?;
    let mac = base64::engine::general_purpose::STANDARD
        .decode(envelope["mac"].as_str().unwrap_or(""))
        .map_err(|_| "Connector authentication failed")?;
    ring::hmac::verify(key, &bytes, &mac).map_err(|_| "Connector authentication failed")?;
    if payload["instance"] != instance {
        return Err("This connector instance is no longer available".into());
    }
    Ok(payload.clone())
}
async fn start(
    root: PathBuf,
    sender: mpsc::Sender<Call>,
    epoch: Arc<AtomicU64>,
    ctx: egui::Context,
) -> Result<Server, String> {
    use base64::Engine;
    use rand::RngCore;
    use zeroize::Zeroizing;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| "Cannot open local connector")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Cannot read connector address")?
        .port();
    let secret = Zeroizing::new({
        let mut bytes = vec![0; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        bytes
    });
    let credential =
        base64::engine::general_purpose::STANDARD.encode(protocol::protect(&secret, false)?);
    let key = Arc::new(ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &secret));
    let instance = uuid::Uuid::new_v4().to_string();
    let bytes = serde_json::to_vec(
        &json!({"schema":1,"port":port,"instance":instance,"credential":credential}),
    )
    .map_err(|_| "Cannot encode connector descriptor")?;
    let mut file =
        tempfile::NamedTempFile::new_in(&root).map_err(|_| "Cannot create connector descriptor")?;
    std::io::Write::write_all(&mut file, &bytes).map_err(|_| "Cannot save connector descriptor")?;
    file.persist(root.join(protocol::RUNTIME_FILE))
        .map_err(|_| "Cannot publish connector descriptor")?;
    let descriptor = Descriptor {
        root,
        instance: instance.clone(),
    };
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        let _descriptor = descriptor;
        let capacity = Arc::new(Semaphore::new(8));
        let mut jobs = tokio::task::JoinSet::new();
        loop {
            let accepted = tokio::select! {biased;_=stop.cancelled()=>break,accepted=listener.accept()=>accepted,done=jobs.join_next(),if !jobs.is_empty()=>{let _=done;continue;}};
            let Ok((socket, address)) = accepted else {
                break;
            };
            if !address.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = capacity.clone().try_acquire_owned() else {
                continue;
            };
            let sender = sender.clone();
            let key = key.clone();
            let instance = instance.clone();
            let epoch = epoch.clone();
            let stop = stop.clone();
            let ctx = ctx.clone();
            jobs.spawn(async move {
                let _permit=permit;
                let (read,mut write)=socket.into_split();let mut read=BufReader::new(read);
                let line=tokio::select!{_=stop.cancelled()=>return,line=tokio::time::timeout(Duration::from_secs(10),protocol::read_line(&mut read))=>line};
                let Ok(Ok(Some(line)))=line else{return;};
                let result=serde_json::from_slice::<Value>(&line).map_err(|_|"Invalid connector envelope".to_owned()).and_then(|data|authenticate(&data,&instance,&key));
                let payload=match result {Ok(payload)=>payload,Err(error)=>{let _=protocol::write_line(&mut write,&protocol::error_result(&error)).await;return;}};
                let name=payload["tool"].as_str().unwrap_or("").to_owned();let args=payload["arguments"].clone();
                if let Err(error)=protocol::validate(&name,&args){let _=protocol::write_line(&mut write,&protocol::error_result(&error)).await;return;}
                let cancel=CancellationToken::new();let _cancel=CancelOnDrop(cancel.clone());
                let (reply,receive)=oneshot::channel();
                let call=Call{name,args,epoch:epoch.load(Ordering::Acquire),cancel:cancel.clone(),reply,created:Instant::now()};
                if sender.try_send(call).is_err(){let _=protocol::write_line(&mut write,&protocol::error_result("AetherDesk's connector queue is busy")).await;return;}
                ctx.request_repaint();
                let mut eof=[0u8;1];
                let response=tokio::select!{biased;
                    _=stop.cancelled()=>protocol::error_result("Connector disabled"),
                    _=cancel.cancelled()=>protocol::error_result("Workspace access was revoked"),
                    _=read.read(&mut eof)=>return,
                    result=receive=>result.unwrap_or_else(|_|protocol::error_result("The request was cancelled")),
                    _=tokio::time::sleep(Duration::from_secs(140))=>protocol::error_result("Approval or action timed out"),
                };
                let _=tokio::time::timeout(Duration::from_secs(10),protocol::write_line(&mut write,&response)).await;
            });
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
    });
    Ok(Server { cancel })
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    enabled: bool,
    #[serde(default = "default_read_only")]
    read_only: bool,
    folder: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            read_only: true,
            folder: String::new(),
        }
    }
}
fn default_read_only() -> bool {
    true
}
enum Event {
    Started(Result<Server, String>),
    Registered(Result<(), String>),
}
pub struct Connector {
    pub open: bool,
    settings: Settings,
    valid: bool,
    error: Option<String>,
    server: Option<Server>,
    starting: bool,
    epoch: Arc<AtomicU64>,
    shares: HashMap<String, Workspace>,
    local_id: String,
    pending: VecDeque<Call>,
    running: Vec<Running>,
    sender: mpsc::Sender<Call>,
    requests: mpsc::Receiver<Call>,
    events: mpsc::Receiver<Event>,
    updates: mpsc::Sender<Event>,
    registering: bool,
}
impl Default for Connector {
    fn default() -> Self {
        let (sender, requests) = mpsc::channel(8);
        let (updates, events) = mpsc::channel(8);
        let loaded = crate::storage::read_json("codex-connector.json").and_then(|value| {
            value
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| "Invalid Codex connector settings; file preserved".into())
        });
        let (settings, valid, error) = match loaded {
            Ok(Some(settings)) => (settings, true, None),
            Ok(None) => (
                Settings {
                    read_only: true,
                    ..Default::default()
                },
                true,
                None,
            ),
            Err(error) => (
                Settings {
                    read_only: true,
                    ..Default::default()
                },
                false,
                Some(error),
            ),
        };
        Self {
            open: false,
            settings,
            valid,
            error,
            server: None,
            starting: false,
            epoch: Arc::new(AtomicU64::new(0)),
            shares: HashMap::new(),
            local_id: format!("windows:{}", uuid::Uuid::new_v4()),
            pending: VecDeque::new(),
            running: Vec::new(),
            sender,
            requests,
            events,
            updates,
            registering: false,
        }
    }
}
pub(crate) fn codex_executable() -> Option<PathBuf> {
    // Resolve an installed executable, never one in the active project folder.
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("codex.exe"))
            .find(|path| path.is_file())
    }) {
        return Some(path);
    }
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("OpenAI/Codex/bin");
    let mut candidates: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("codex.exe"))
        .filter(|path| path.is_file())
        .collect();
    candidates.sort_by_key(|path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok()));
    candidates.pop()
}
fn same_connection(a: &Workspace, b: &Workspace) -> bool {
    match (a, b) {
        (Workspace::Ssh { session: a, .. }, Workspace::Ssh { session: b, .. }) => {
            a.connection_id() == b.connection_id()
        }
        (Workspace::Local(a), Workspace::Local(b)) => a == b,
        _ => false,
    }
}
impl Connector {
    fn revoke(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        for call in self.pending.drain(..) {
            call.cancel.cancel();
            let _ = call.reply.send(protocol::error_result(
                "Workspace access changed; request cancelled",
            ));
        }
        for action in self.running.drain(..) {
            action.cancel.cancel();
        }
    }
    pub fn stop(&mut self) {
        self.settings.enabled = false;
        self.revoke();
        self.shares.clear();
        self.server = None;
    }
    fn save(&mut self) {
        if self.valid {
            if let Err(error) =
                crate::storage::write_json("codex-connector.json", &json!(self.settings))
            {
                self.error = Some(error);
            }
        }
    }
    fn workspace(&self, call: &Call) -> Result<Workspace, String> {
        if !self.settings.enabled
            || self.server.is_none()
            || call.cancel.is_cancelled()
            || call.reply.is_closed()
            || call.epoch != self.epoch.load(Ordering::Acquire)
        {
            return Err("Connector request is no longer authorized".into());
        }
        let read_only = protocol::validate(&call.name, &call.args)?;
        if !read_only && self.settings.read_only {
            return Err("Read-only mode blocks commands and file writes. Change it explicitly in AetherDesk's Codex connector settings".into());
        }
        let id = call.args["workspace_id"]
            .as_str()
            .ok_or("Choose an explicitly shared workspace_id")?;
        let workspace=self.shares.get(id).filter(|w|w.is_available()).cloned().ok_or("This workspace is not shared or its connection is closed. Ask the user to share it in AetherDesk")?;
        Ok(workspace)
    }
    fn dispatch(
        &mut self,
        call: Call,
        workspace: Workspace,
        runtime: &Connections,
        ctx: &egui::Context,
    ) {
        let cancel = call.cancel.clone();
        let finished = CancellationToken::new();
        self.running.push(Running {
            cancel: cancel.clone(),
            finished: finished.clone(),
        });
        let ctx = ctx.clone();
        runtime.spawn(async move{
            let _finished = CancelOnDrop(finished);
            let work=async {
                if call.name=="aetherdesk_terminal_text" || call.name=="aetherdesk_capture_screen" {
                    let name=if call.name=="aetherdesk_terminal_text"{"app_terminal_text"}else{"app_capture_screen"};
                    let value=crate::app_tools::execute(name.into(),json!({"connection_id":call.args["workspace_id"]})).await?;
                    if let Some(uri)=value["image_data_url"].as_str(){
                        if let Some(data)=uri.strip_prefix("data:image/png;base64,") {return Ok(json!({"content":[{"type":"image","mimeType":"image/png","data":data}],"isError":false}));}
                    }
                    return Ok(protocol::text_result(value));
                }
                let mut args=call.args.clone();args.as_object_mut().unwrap().remove("workspace_id");
                let name=call.name.strip_prefix("aetherdesk_").ok_or("Unknown tool")?.to_owned();
                let value=ai_tools::execute(workspace,name,args).await?;
                Ok::<_,String>(protocol::text_result(value))
            };
            let result=tokio::select!{biased;_=cancel.cancelled()=>Err("Connector action cancelled".into()),result=work=>result};
            let _=call.reply.send(result.unwrap_or_else(|error|protocol::error_result(&error)));
            // Completion is separate from revocation: cancelling the request
            // here races the transport's reply and can discard a valid result.
            ctx.request_repaint();
        });
    }
    pub fn tick(&mut self, ctx: &egui::Context, runtime: &Connections, targets: &[Target]) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Started(result) => {
                    self.starting = false;
                    if self.settings.enabled {
                        match result {
                            Ok(server) => self.server = Some(server),
                            Err(error) => {
                                self.settings.enabled = false;
                                self.error = Some(error);
                                self.save();
                            }
                        }
                    }
                }
                Event::Registered(result) => {
                    self.registering = false;
                    self.error=Some(match result{Ok(())=>"Connected in Codex settings. Refresh its MCP tools or start a new Codex chat if the current chat keeps an older tool catalog.".into(),Err(error)=>error});
                }
            }
        }
        if self.settings.enabled && self.server.is_none() && !self.starting && self.valid {
            match crate::storage::root() {
                Ok(root) => {
                    self.starting = true;
                    let sender = self.sender.clone();
                    let events = self.updates.clone();
                    let epoch = self.epoch.clone();
                    let ctx = ctx.clone();
                    runtime.spawn(async move {
                        let result = start(root, sender, epoch, ctx.clone()).await;
                        let _ = events.send(Event::Started(result)).await;
                        ctx.request_repaint();
                    });
                }
                Err(error) => self.error = Some(error),
            }
        }
        let stale = self.shares.iter().any(|(id, workspace)| {
            id != &self.local_id
                && !targets
                    .iter()
                    .any(|target| &target.id == id && same_connection(workspace, &target.workspace))
        });
        if stale {
            self.revoke();
            self.shares.retain(|id, workspace| {
                id == &self.local_id
                    || targets.iter().any(|target| {
                        &target.id == id && same_connection(workspace, &target.workspace)
                    })
            });
        }
        self.running
            .retain(|action| !action.finished.is_cancelled());
        while let Ok(call) = self.requests.try_recv() {
            if call.cancel.is_cancelled() || call.reply.is_closed() {
                continue;
            }
            if !self.settings.enabled
                || self.server.is_none()
                || call.epoch != self.epoch.load(Ordering::Acquire)
            {
                let _ = call
                    .reply
                    .send(protocol::error_result("Connector access changed"));
                continue;
            }
            if call.name == "aetherdesk_status" {
                let _=call.reply.send(protocol::text_result(json!({"connected":true,"read_only":self.settings.read_only,"shared_workspaces":self.shares.len(),"version":env!("CARGO_PKG_VERSION")})));
                continue;
            }
            if call.name == "aetherdesk_list_connections" {
                let list:Vec<_>=self.shares.iter().map(|(id,workspace)|json!({"workspace_id":id,"workspace":workspace.label(),"target":if matches!(workspace,Workspace::Local(_)){"windows"}else{"ssh"},"shell":if matches!(workspace,Workspace::Local(_)){"PowerShell 5.1"}else{"sh -lc"}})).collect();
                let _ = call.reply.send(protocol::text_result(json!(list)));
                continue;
            }
            match self.workspace(&call) {
                Err(error) => {
                    let _ = call.reply.send(protocol::error_result(&error));
                }
                Ok(workspace) => {
                    if protocol::validate(&call.name, &call.args) == Ok(true) {
                        self.dispatch(call, workspace, runtime, ctx);
                    } else if self.pending.len() < 8 {
                        self.pending.push_back(call);
                    } else {
                        let _ = call
                            .reply
                            .send(protocol::error_result("Too many pending approvals"));
                    }
                }
            }
        }
        self.pending.retain(|call| {
            !call.cancel.is_cancelled()
                && !call.reply.is_closed()
                && call.created.elapsed() < Duration::from_secs(120)
        });
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.ui(ctx, runtime, targets);
    }
    fn register(&mut self, runtime: &Connections, ctx: &egui::Context) {
        let executable = match std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("aetherdesk-codex.exe")))
        {
            Some(exe) if exe.is_file() => exe,
            _ => {
                self.error =
                    Some("The Codex companion is missing. Use a complete Build 9 package.".into());
                return;
            }
        };
        let directory = match crate::storage::root() {
            Ok(root) => root,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.registering = true;
        let events = self.updates.clone();
        let ctx = ctx.clone();
        runtime.spawn(async move{
            let Some(executable_codex)=codex_executable() else{let _=events.send(Event::Registered(Err("Install the Codex CLI or desktop app before connecting.".into()))).await;ctx.request_repaint();return;};
            let mut command=tokio::process::Command::new(executable_codex);
            command.args(["mcp","add","aetherdesk","--env"]).arg(format!("AETHERDESK_CONNECTOR_DIRECTORY={}",directory.display())).arg("--").arg(executable).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
            #[cfg(windows)]command.creation_flags(0x08000000);
            let result=match tokio::time::timeout(Duration::from_secs(20),command.status()).await{Ok(Ok(status))if status.success()=>Ok(()),_=>Err("Could not register with Codex. Ensure the Codex CLI is installed and available, then retry.".into())};
            let _=events.send(Event::Registered(result)).await;ctx.request_repaint();
        });
    }
    fn ui(&mut self, ctx: &egui::Context, runtime: &Connections, targets: &[Target]) {
        if self.open {
            let mut open = true;
            egui::Window::new("Connect Codex").open(&mut open).fixed_size(crate::app_layout::dialog_size(ctx,egui::vec2(520.0,600.0))).resizable(false).show(ctx,|ui|{
                ui.set_max_width((ctx.content_rect().width()-48.0).clamp(180.0,480.0));
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-160.0).max(160.0)).show(ui,|ui|{
                    ui.label("Use your Codex chat with workspaces explicitly shared here. No separate model API key is required by this connector.");
                    if ui.add_enabled(self.valid,egui::Checkbox::new(&mut self.settings.enabled,"Enable local Codex connector")).changed(){if !self.settings.enabled{self.stop();}self.save();}
                    if ui.checkbox(&mut self.settings.read_only,"Read-only · block commands and file writes").changed(){self.revoke();self.save();}
                    ui.separator();ui.strong("Shared workspaces");
                    ui.label("Shared connections expose terminal output, root-folder files and remote screenshots to Codex.");
                    for target in targets{
                        let mut share=self.shares.contains_key(&target.id);
                        if ui.add_enabled(self.settings.enabled,egui::Checkbox::new(&mut share,format!("{} · {}",target.name,target.kind))).on_hover_text(target.workspace.label()).changed(){self.revoke();if share{self.shares.insert(target.id.clone(),target.workspace.clone());}else{self.shares.remove(&target.id);}}
                        if let Some(workspace)=self.shares.get(&target.id){ui.add(egui::Label::new(workspace.label()).truncate()).on_hover_text(workspace.label());}
                    }
                    if targets.is_empty(){ui.label("Connect a host to share it here.");}
                    ui.separator();ui.label("Windows folder");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.folder).hint_text("Folder path").desired_width(f32::INFINITY));
                    if ui.add_enabled(self.settings.enabled,egui::Button::new(if self.shares.contains_key(&self.local_id){"Change shared folder"}else{"Share this folder"})).clicked(){
                        match std::fs::canonicalize(&self.settings.folder){Ok(path)if path.is_dir()=>{self.revoke();self.settings.folder=ai_tools::display_path(&path);self.shares.insert(self.local_id.clone(),Workspace::Local(path));self.save();},_=>self.error=Some("Choose an existing Windows folder".into())}
                    }
                    if self.shares.contains_key(&self.local_id)&&ui.button("Stop sharing Windows folder").clicked(){self.revoke();self.shares.remove(&self.local_id);}
                    ui.label("All shares reset when AetherDesk closes. Commands run with the account's permissions; each command or file write requires Allow once below.");
                    ui.separator();
                    if ui.add_enabled(self.server.is_some()&&!self.registering,egui::Button::new("Connect to installed Codex")).clicked(){self.register(runtime,ctx);}
                    ui.label(if self.starting{"Starting…"}else if self.server.is_some(){"Local connector is available"}else{"Connector is off"});
                    if let Some(error)=&self.error{ui.add(egui::Label::new(error).wrap());}
                });
            });
            self.open = open;
        }
        if let Some(call) = self.pending.front() {
            let mut decision = None;
            egui::Window::new("Codex requests permission")
                .id(egui::Id::new("codex-approval"))
                .collapsible(false)
                .resizable(false)
                .fixed_size(crate::app_layout::dialog_size(
                    ctx,
                    egui::vec2(520.0, 380.0),
                ))
                .show(ctx, |ui| {
                    ui.strong(&call.name);
                    if let Ok(workspace) = self.workspace(call) {
                        ui.add(egui::Label::new(workspace.instructions()).wrap());
                    }
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    serde_json::to_string_pretty(&call.args).unwrap_or_default(),
                                )
                                .wrap()
                                .selectable(true),
                            );
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Allow once").clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Deny").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if let Some(allow) = decision {
                if let Some(call) = self.pending.pop_front() {
                    if allow {
                        match self.workspace(&call) {
                            Ok(workspace) => self.dispatch(call, workspace, runtime, ctx),
                            Err(error) => {
                                let _ = call.reply.send(protocol::error_result(&error));
                            }
                        }
                    } else {
                        let _ = call.reply.send(protocol::error_result(
                            "The user denied this action. Do not retry it without a new request.",
                        ));
                    }
                }
            }
        }
    }
}
impl Drop for Connector {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_reads_stay_in_the_shared_root_and_writes_require_a_physical_approval_click() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(root.path(), || {
            let shared = root.path().join("shared");
            std::fs::create_dir(&shared).unwrap();
            std::fs::write(shared.join("input.txt"), b"fixture-only text").unwrap();
            let mut connector = Connector::default();
            connector.settings.enabled = true;
            connector.server = Some(Server {
                cancel: CancellationToken::new(),
            });
            connector.shares.insert(
                connector.local_id.clone(),
                Workspace::Local(std::fs::canonicalize(&shared).unwrap()),
            );
            let runtime = Connections::new().unwrap();
            let ctx = egui::Context::default();
            let (read, mut result) = make_call(
                "aetherdesk_read_file",
                json!({"workspace_id":connector.local_id,"path":"input.txt"}),
                0,
            );
            let read_cancel = read.cancel.clone();
            connector.sender.try_send(read).unwrap();
            connector.tick(&ctx, &runtime, &[]);
            let deadline = Instant::now() + Duration::from_secs(5);
            let read = loop {
                if let Ok(result) = result.try_recv() {
                    break result;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(read["isError"], false);
            assert!(
                !read_cancel.is_cancelled(),
                "Completion must not revoke the transport reply"
            );
            connector.tick(&ctx, &runtime, &[]);
            assert!(
                connector.running.is_empty(),
                "Completed actions must leave no tracking entries"
            );
            assert!(read["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("fixture-only text"));
            let (blocked, mut result) = make_call(
                "aetherdesk_write_file",
                json!({"workspace_id":connector.local_id,"path":"output.txt","text":"approved text","expected_revision":""}),
                0,
            );
            connector.sender.try_send(blocked).unwrap();
            connector.tick(&ctx, &runtime, &[]);
            assert_eq!(result.try_recv().unwrap()["isError"], true);
            assert!(!shared.join("output.txt").exists());
            connector.settings.read_only = false;
            let (write, mut result) = make_call(
                "aetherdesk_write_file",
                json!({"workspace_id":connector.local_id,"path":"output.txt","text":"approved text","expected_revision":""}),
                0,
            );
            connector.sender.try_send(write).unwrap();
            let render = |connector: &mut Connector, events| {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 700.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| connector.tick(ctx, &runtime, &[]),
                );
                output
                    .shapes
                    .into_iter()
                    .find_map(|shape| match shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == "Allow once" => {
                            Some(text.pos + text.galley.size() / 2.0)
                        }
                        _ => None,
                    })
            };
            for _ in 0..3 {
                render(&mut connector, vec![]);
            }
            assert!(result.try_recv().is_err());
            assert!(!shared.join("output.txt").exists());
            let point = render(&mut connector, vec![]).unwrap();
            for pressed in [true, false] {
                render(
                    &mut connector,
                    vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let wrote = loop {
                if let Ok(result) = result.try_recv() {
                    break result;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(wrote["isError"], false, "{wrote}");
            assert_eq!(
                std::fs::read(shared.join("output.txt")).unwrap(),
                b"approved text"
            );
            let (escape, mut result) = make_call(
                "aetherdesk_read_file",
                json!({"workspace_id":connector.local_id,"path":"../codex-connector.json"}),
                0,
            );
            connector.sender.try_send(escape).unwrap();
            connector.tick(&ctx, &runtime, &[]);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(result) = result.try_recv() {
                    assert_eq!(result["isError"], true);
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    }
    #[test]
    fn connector_authentication_rejects_tampering_and_other_instances() {
        use base64::Engine;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"fixture-key");
        let payload = json!({"instance":"one","tool":"aetherdesk_status","arguments":{}});
        let signature = base64::engine::general_purpose::STANDARD
            .encode(ring::hmac::sign(&key, &serde_json::to_vec(&payload).unwrap()).as_ref());
        let mut envelope = json!({"payload":payload,"mac":signature});
        assert!(authenticate(&envelope, "one", &key).is_ok());
        assert!(authenticate(&envelope, "two", &key).is_err());
        envelope["payload"]["tool"] = json!("aetherdesk_run_command");
        assert!(authenticate(&envelope, "one", &key).is_err());
    }
    fn make_call(name: &str, args: Value, epoch: u64) -> (Call, oneshot::Receiver<Value>) {
        let (reply, receive) = oneshot::channel();
        (
            Call {
                name: name.into(),
                args,
                epoch,
                cancel: CancellationToken::new(),
                reply,
                created: Instant::now(),
            },
            receive,
        )
    }
    #[test]
    fn connector_shares_are_explicit_and_revocation_cancels_pending_actions() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(root.path(), || {
            let mut connector = Connector::default();
            assert!(!connector.settings.enabled);
            assert!(connector.settings.read_only);
            connector.settings.enabled = true;
            connector.server = Some(Server {
                cancel: CancellationToken::new(),
            });
            let (call, _receive) = make_call(
                "aetherdesk_read_file",
                json!({"workspace_id":"one","path":"file"}),
                0,
            );
            assert!(connector.workspace(&call).is_err());
            connector
                .shares
                .insert("one".into(), Workspace::Local(root.path().to_owned()));
            assert!(connector.workspace(&call).is_ok());
            let (action, mut result) = make_call(
                "aetherdesk_write_file",
                json!({"workspace_id":"one","path":"file","text":"x","expected_revision":""}),
                0,
            );
            assert!(
                connector.workspace(&action).is_err(),
                "read-only mode must reject writes before approval"
            );
            connector.settings.read_only = false;
            assert!(connector.workspace(&action).is_ok());
            connector.pending.push_back(action);
            connector.revoke();
            assert!(result.try_recv().unwrap()["isError"] == true);
            assert!(
                connector.workspace(&call).is_err(),
                "old queued requests cannot use a new policy"
            );
            connector.stop();
            assert!(!connector.settings.enabled);
            assert!(connector.shares.is_empty());
        });
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn authenticated_gui_transport_handles_results_cancellation_and_descriptor_ownership() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let (sender, mut receive) = mpsc::channel(8);
        let epoch = Arc::new(AtomicU64::new(4));
        let server = start(root.path().to_owned(), sender, epoch, ctx)
            .await
            .unwrap();
        let path = root.path().to_owned();
        let work =
            tokio::spawn(
                async move { protocol::forward(&path, "aetherdesk_status", json!({})).await },
            );
        let call = tokio::time::timeout(Duration::from_secs(3), receive.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(call.epoch, 4);
        let _ = call
            .reply
            .send(protocol::text_result(json!({"connected":true})));
        assert!(work.await.unwrap().unwrap()["isError"] == false);
        let path = root.path().to_owned();
        let work =
            tokio::spawn(
                async move { protocol::forward(&path, "aetherdesk_status", json!({})).await },
            );
        let call = tokio::time::timeout(Duration::from_secs(3), receive.recv())
            .await
            .unwrap()
            .unwrap();
        work.abort();
        let _ = work.await;
        tokio::time::timeout(Duration::from_secs(3), call.cancel.cancelled())
            .await
            .unwrap();
        assert!(call.reply.is_closed());
        // A closing older instance must not delete a newer instance's pointer.
        let descriptor = root.path().join(protocol::RUNTIME_FILE);
        std::fs::write(&descriptor, b"{\"instance\":\"another-instance\"}").unwrap();
        drop(server);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(descriptor.exists());
    }
}
