//! LSP on the document's original authenticated SSH connection. Servers and
//! arguments are explicitly selected by the user, as in the reference editor.
use crate::editor_transport::Transport;
use ad_ssh::{LanguageServerConfig, SshSession};
#[derive(Clone)]
pub enum Action {
    Edit { workspace: Value, version: u64 },
    Definition { uri: String, range: Value },
}
use eframe::egui;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub struct Language {
    server: Option<Transport>,
    receiver: Option<mpsc::Receiver<Result<Transport, String>>>,
    executable: String,
    arguments: String,
    language: String,
    uri: String,
    root: String,
    next: u64,
    version: u64,
    opened: bool,
    pending: HashMap<u64, (String, u64, Instant, usize)>,
    changed: Option<Instant>,
    diagnostics: Vec<Value>,
    completions: Vec<Value>,
    completion_version: u64,
    completion_cursor: usize,
    hover: String,
    error: Option<String>,
    show: bool,
    rename: String,
    actions: VecDeque<Action>,
    settings_key: Option<String>,
}
impl Default for Language {
    fn default() -> Self {
        Self {
            server: None,
            receiver: None,
            executable: String::new(),
            arguments: "[]".into(),
            language: "plaintext".into(),
            uri: String::new(),
            root: String::new(),
            next: 0,
            version: 1,
            opened: false,
            pending: HashMap::new(),
            changed: None,
            diagnostics: vec![],
            completions: vec![],
            completion_version: 0,
            completion_cursor: 0,
            hover: String::new(),
            error: None,
            show: false,
            rename: String::new(),
            actions: VecDeque::new(),
            settings_key: None,
        }
    }
}
impl Language {
    pub fn for_path(path: &str) -> Self {
        let mut this = Self::default();
        this.language =
            crate::editor_tools::language_id(&crate::editor_tools::language(path)).into();
        this
    }
    pub fn take_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }
    pub fn toggle(&mut self) {
        self.show = !self.show;
    }
    fn send(&mut self, value: Value) -> bool {
        if let Some(server) = &self.server {
            if let Err(error) = server.send(value) {
                self.error = Some(error.to_string());
                return false;
            }
            return true;
        }
        false
    }
    fn request(&mut self, method: &str, params: Value) {
        if self.pending.len() >= 64 {
            self.error = Some("Too many language requests".into());
            return;
        }
        self.next += 1;
        let id = self.next;
        self.pending.insert(
            id,
            (
                method.into(),
                self.version,
                Instant::now(),
                self.completion_cursor,
            ),
        );
        if !self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})) {
            self.pending.remove(&id);
        }
    }
    pub fn changed(&mut self) {
        self.version += 1;
        self.changed = Some(Instant::now());
        self.completions.clear();
        self.hover.clear();
        self.diagnostics.clear();
    }
    pub fn revision(&self) -> u64 {
        self.version
    }
    fn flush(&mut self, text: &str) {
        if self.changed.is_some() && self.opened {
            if self.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":self.uri,"version":self.version},"contentChanges":[{"text":text}]}})){self.changed=None;}else{self.stop();}
        }
    }
    pub fn saved(&mut self, text: &str) {
        self.flush(text);
        if self.opened {
            self.send(json!({"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":self.uri}}}));
        }
    }
    pub fn stop(&mut self) {
        if self.opened {
            self.send(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":self.uri}}}));
        }
        if let Some(server) = self.server.take() {
            server.stop();
        }
        self.receiver = None;
        self.opened = false;
        self.pending.clear();
        self.actions.clear();
        self.completions.clear();
        self.diagnostics.clear();
    }
    fn poll(&mut self, text: &str, ctx: &egui::Context) {
        if let Some(receiver) = &mut self.receiver {
            if let Ok(result) = receiver.try_recv() {
                self.receiver = None;
                match result {
                    Ok(server) => {
                        self.server = Some(server);
                        self.request("initialize",json!({"processId":null,"clientInfo":{"name":"AetherDesk Rust"},"rootUri":self.root,"capabilities":{"general":{"positionEncodings":["utf-16"]},"workspace":{"configuration":true,"workspaceEdit":{"documentChanges":true,"resourceOperations":[]}},"textDocument":{"completion":{"completionItem":{"snippetSupport":false,"insertReplaceSupport":true}},"publishDiagnostics":{"versionSupport":true}}}}));
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        let Some(server) = &self.server else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(150));
        let state = server.poll();
        if state["state"] != "running" {
            self.error = Some(
                state["error"]
                    .as_str()
                    .unwrap_or("Language server stopped")
                    .into(),
            );
            self.stop();
            return;
        }
        if let Some(messages) = state["messages"].as_array() {
            for message in messages {
                if message["method"] == "textDocument/publishDiagnostics" {
                    let p = &message["params"];
                    if p["uri"] == self.uri
                        && p["version"].as_u64().is_none_or(|v| v == self.version)
                    {
                        self.diagnostics = p["diagnostics"].as_array().cloned().unwrap_or_default();
                    }
                } else if let Some(method) = message["method"].as_str() {
                    if !message["id"].is_null() {
                        let response = if method == "workspace/configuration" {
                            json!({"jsonrpc":"2.0","id":message["id"],"result":vec![Value::Null;message["params"]["items"].as_array().map_or(0,Vec::len)]})
                        } else {
                            json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Client method is not supported"}})
                        };
                        self.send(response);
                    }
                } else if let Some(id) = message["id"].as_u64() {
                    if let Some((method, version, _, request_cursor)) = self.pending.remove(&id) {
                        if !message["error"].is_null() {
                            self.error = Some(
                                message["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Language request failed")
                                    .into(),
                            );
                            continue;
                        }
                        if method == "initialize" {
                            let encoding = message["result"]["capabilities"]["positionEncoding"]
                                .as_str()
                                .unwrap_or("utf-16");
                            if encoding != "utf-16" {
                                self.error = Some(
                                    "This language server did not negotiate UTF-16 positions"
                                        .into(),
                                );
                                self.stop();
                                return;
                            }
                            self.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}));
                            self.opened = self.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":self.uri,"languageId":self.language,"version":self.version,"text":text}}}));
                            if !self.opened {
                                self.stop();
                                return;
                            }
                            self.changed = None;
                        } else if version == self.version {
                            if method == "textDocument/formatting" {
                                self.actions.push_back(Action::Edit {
                                    workspace: json!({"changes":{(self.uri.clone()):message["result"]}}),
                                    version,
                                });
                            }
                            if method == "textDocument/rename" && message["result"].is_object() {
                                self.actions.push_back(Action::Edit {
                                    workspace: message["result"].clone(),
                                    version,
                                });
                            }
                            if method == "textDocument/definition" {
                                let result = &message["result"];
                                let item =
                                    result.as_array().and_then(|a| a.first()).unwrap_or(result);
                                if let Some(uri) =
                                    item["uri"].as_str().or_else(|| item["targetUri"].as_str())
                                {
                                    self.actions.push_back(Action::Definition {
                                        uri: uri.into(),
                                        range: if item["range"].is_object() {
                                            item["range"].clone()
                                        } else {
                                            item["targetSelectionRange"].clone()
                                        },
                                    });
                                }
                            }
                            if method == "textDocument/completion" {
                                self.completions = message["result"]
                                    .as_array()
                                    .or_else(|| message["result"]["items"].as_array())
                                    .cloned()
                                    .unwrap_or_default();
                                self.completion_version = version;
                                self.completion_cursor = request_cursor;
                            }
                            if method == "textDocument/hover" {
                                self.hover = message["result"]["contents"]
                                    .as_str()
                                    .or_else(|| message["result"]["contents"]["value"].as_str())
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| message["result"]["contents"].to_string());
                            }
                        }
                    }
                }
            }
        }
        let expired = self
            .pending
            .iter()
            .filter(|(_, (_, _, at, _))| at.elapsed() > Duration::from_secs(20))
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            self.pending.remove(&id);
            self.send(json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}));
            self.error = Some("Language request timed out".into());
        }
        if self
            .changed
            .is_some_and(|at| at.elapsed() > Duration::from_millis(350))
        {
            self.flush(text);
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &crate::connection::Connections,
        remote: Option<(Arc<SshSession>, String)>,
        local: Option<std::path::PathBuf>,
        text: &mut String,
        cursor: usize,
        indent: usize,
        tabs: bool,
        editor_focused: bool,
    ) {
        self.poll(text, ui.ctx());
        if self.opened && editor_focused {
            let completion =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Space));
            let definition = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F12));
            if completion || definition {
                self.show = true;
                self.flush(text);
                self.completion_cursor = cursor;
                self.request(
                    if completion {
                        "textDocument/completion"
                    } else {
                        "textDocument/definition"
                    },
                    json!({"textDocument":{"uri":self.uri},"position":position(text,cursor)}),
                );
            }
            if ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::ALT | egui::Modifiers::SHIFT, egui::Key::F)
            }) {
                self.show = true;
                self.flush(text);
                self.request("textDocument/formatting",json!({"textDocument":{"uri":self.uri},"options":{"tabSize":indent,"insertSpaces":!tabs}}));
            }
        }
        if !self.show {
            return;
        }
        let available = remote.is_some() || local.is_some();
        let key = format!(
            "{}:{}",
            remote.as_ref().map_or("local".into(), |(session, _)| {
                crate::working_copies::identity(session)
            }),
            self.language
        );
        if self.settings_key.is_none() {
            match crate::storage::read_json("editor-language-servers.json") {
                Ok(Some(config)) => {
                    if let Some(config) = config.get(&key) {
                        self.executable = config["executable"].as_str().unwrap_or("").into();
                        self.arguments = config["arguments"].as_str().unwrap_or("[]").into();
                    }
                }
                Err(error) => self.error = Some(error),
                _ => {}
            }
            self.settings_key = Some(key.clone());
        }
        egui::CollapsingHeader::new("Language service").default_open(true).show(ui,|ui|{
            if self.server.is_none()&&self.receiver.is_none(){ let mut enter=false;
                ui.label(if remote.is_some(){"Server executable on this SSH host"}else{"Server executable on this computer"});let response=ui.text_edit_singleline(&mut self.executable); enter |= crate::dialog::field_submit(ui, &response);
                ui.label("Arguments (JSON array)");let response=ui.text_edit_singleline(&mut self.arguments); enter |= crate::dialog::field_submit(ui, &response);
                ui.label("Language ID, e.g. python, rust, cpp");let response=ui.text_edit_singleline(&mut self.language); enter |= crate::dialog::field_submit(ui, &response);
                if ui.button("Save server configuration").clicked() || enter {
                    let result=(||{serde_json::from_str::<Vec<String>>(&self.arguments).map_err(|e|e.to_string())?;let mut configs=crate::storage::read_json("editor-language-servers.json")?.unwrap_or_else(||json!({}));if !configs.is_object(){return Err("Language settings are damaged; existing file preserved".into())}configs[key.clone()]=json!({"executable":self.executable,"arguments":self.arguments});crate::storage::write_json("editor-language-servers.json",&configs)})();
                    if let Err(error)=result{self.error=Some(error);}
                }
                if ui.add_enabled(available,egui::Button::new("Start language server")).clicked(){
                    let result=serde_json::from_str::<Vec<String>>(&self.arguments).map_err(|_|"Arguments must be a JSON array of strings".to_owned());
                    match result {Ok(arguments)=>{
                        self.error=None;
                        let (sender,receiver)=mpsc::channel(1);self.receiver=Some(receiver);let ctx=ui.ctx().clone();
                        if let Some((session,path))=remote.clone(){
                            let parent=path.rsplit_once('/').map(|p|if p.0.is_empty(){"/"}else{p.0}).unwrap_or("/").to_owned();
                            self.uri=file_uri(&path);self.root=file_uri(&parent);
                            let config=LanguageServerConfig{executable:self.executable.clone(),arguments,working_directory:Some(parent)};
                            runtime.spawn(async move{let result=session.start_language_server(config).await.map(Transport::Remote).map_err(|e|e.to_string());let _=sender.send(result).await;ctx.request_repaint();});
                        }else if let Some(path)=local.as_ref(){
                            let parent=path.parent().unwrap_or(path).to_path_buf();
                            match (reqwest::Url::from_file_path(path),reqwest::Url::from_directory_path(&parent)) {
                                (Ok(uri),Ok(root))=>{self.uri=uri.to_string();self.root=root.to_string();let executable=self.executable.clone();runtime.spawn(async move{let result=Transport::local(executable,arguments,parent).await;let _=sender.send(result).await;ctx.request_repaint();});},
                                _=>{self.receiver=None;self.error=Some("Invalid local document URI".into());}
                            }
                        }
                    },Err(error)=>self.error=Some(error)}
                }

            }else{
                if ui.button("Stop language server").clicked(){self.stop();}
                ui.horizontal(|ui|{for (label,method) in [("Completion","textDocument/completion"),("Hover","textDocument/hover"),("Definition","textDocument/definition")]{if ui.add_enabled(self.opened,egui::Button::new(label)).clicked(){self.flush(text);self.completion_cursor=cursor;self.request(method,json!({"textDocument":{"uri":self.uri},"position":position(text,cursor)}));}}});
            }
            if self.opened {
                ui.horizontal(|ui|{
                    if ui.button("Format document").clicked(){self.flush(text);self.request("textDocument/formatting",json!({"textDocument":{"uri":self.uri},"options":{"tabSize":indent,"insertSpaces":!tabs}}));}
                    ui.label("New symbol name");let response=ui.text_edit_singleline(&mut self.rename); let enter=crate::dialog::field_submit(ui,&response);
                    if ui.add_enabled(!self.rename.trim().is_empty(),egui::Button::new("Rename symbol")).clicked() || (enter && !self.rename.trim().is_empty()){self.flush(text);self.request("textDocument/rename",json!({"textDocument":{"uri":self.uri},"position":position(text,cursor),"newName":self.rename}));}
                });
                ui.label("Formatting and rename changes are reviewed before applying. Save remains explicit.");
            }
            if let Some(error)=&self.error{ui.label(error);}
            if !self.hover.is_empty(){ui.add(egui::Label::new(&self.hover).selectable(true));}
            let mut selected=None;
            egui::ScrollArea::vertical().id_salt("language-results").max_height(180.0).show(ui,|ui|{
                for (i,completion) in self.completions.iter().take(100).enumerate(){if ui.button(completion["label"].as_str().unwrap_or("Completion")).clicked(){selected=Some(i);}}
                for diagnostic in self.diagnostics.iter().take(200){ui.label(format!("{}:{} {}",diagnostic["range"]["start"]["line"].as_u64().unwrap_or(0)+1,diagnostic["range"]["start"]["character"].as_u64().unwrap_or(0)+1,diagnostic["message"].as_str().unwrap_or("")));}
            });
            if let Some(index)=selected{if self.completion_version==self.version{match apply_completion(text,self.completion_cursor,&self.completions[index]){Ok(())=>self.changed(),Err(error)=>self.error=Some(error)}}}
        });
    }
}
impl Drop for Language {
    fn drop(&mut self) {
        self.stop();
    }
}
fn file_uri(path: &str) -> String {
    let mut url = reqwest::Url::parse("file:///").expect("literal URL");
    url.set_path(path);
    url.to_string()
}
fn position(text: &str, cursor: usize) -> Value {
    let mut line = 0;
    let mut character = 0;
    for c in text.chars().take(cursor) {
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16();
        }
    }
    json!({"line":line,"character":character})
}
fn offset(text: &str, position: &Value) -> Result<usize, String> {
    crate::editor_tools::utf16_offset(text, position)
}
fn apply_completion(text: &mut String, cursor: usize, item: &Value) -> Result<(), String> {
    if item["insertTextFormat"] == 2 {
        return Err("This completion requires snippet or additional-edit support".into());
    }
    let edit = &item["textEdit"];
    let (start, end, replacement) = if edit.is_object() {
        let range = if edit["range"].is_object() {
            &edit["range"]
        } else {
            &edit["insert"]
        };
        (
            offset(text, &range["start"])?,
            offset(text, &range["end"])?,
            edit["newText"].as_str().ok_or("Completion text missing")?,
        )
    } else {
        let at = text
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        let start = text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
            .last()
            .map_or(at, |(i, _)| i);
        (
            start,
            at,
            item["insertText"]
                .as_str()
                .or_else(|| item["label"].as_str())
                .ok_or("Completion text missing")?,
        )
    };
    let mut edits = item["additionalTextEdits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut changes = Vec::new();
    for edit in edits.drain(..) {
        changes.push((
            offset(text, &edit["range"]["start"])?..offset(text, &edit["range"]["end"])?,
            edit["newText"]
                .as_str()
                .ok_or("Missing edit text")?
                .to_owned(),
        ));
    }
    changes.push((start..end, replacement.to_owned()));
    *text = crate::editor_tools::apply_byte_edits(text, changes)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_offsets_preserve_non_bmp_characters() {
        let text = "a😀b\nnext";
        assert_eq!(position(text, 2), json!({"line":0,"character":3}));
        assert_eq!(offset(text, &json!({"line":0,"character":3})).unwrap(), 5);
        assert!(offset(text, &json!({"line":0,"character":2})).is_err());
    }
    #[test]
    fn completion_replaces_only_the_server_range() {
        let mut text = "hello world".to_owned();
        apply_completion(&mut text,5,&json!({"textEdit":{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":5}},"newText":"hi"}})).unwrap();
        assert_eq!(text, "hi world");
    }
    #[test]
    fn additional_completion_edits_are_atomic_and_undoable_by_the_editor() {
        let mut text = "foo\ncall fo".to_owned();
        apply_completion(&mut text,11,&json!({"insertText":"foo","additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"use foo"}]})).unwrap();
        assert_eq!(text, "use foo\ncall foo");
        let original = text.clone();
        let result = apply_completion(
            &mut text,
            3,
            &json!({"textEdit":{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},"newText":"x"},"additionalTextEdits":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"newText":"y"}]}),
        );
        assert!(result.is_err());
        assert_eq!(text, original);
    }
}
