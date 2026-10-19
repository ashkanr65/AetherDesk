use crate::{
    ai_tools::{self, Workspace},
    connection::Connections,
    storage,
};
use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    io::Write,
    path::PathBuf,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    schema_version: u8,
    id: String,
    title: String,
    description: String,
    version: String,
    target: String,
    script: String,
    #[serde(default)]
    parameters: Vec<Parameter>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Parameter {
    name: String,
    label: String,
    #[serde(default = "required")]
    required: bool,
    default_value: Option<String>,
}
fn required() -> bool {
    true
}
fn validate_id(id: &str) -> Result<(), String> {
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if id.is_empty()
        || id.len() > 64
        || !id.as_bytes()[0].is_ascii_lowercase()
        || id.split('-').any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
        || reserved.contains(&id)
    {
        return Err("Tool ID must be a lowercase name such as project-summary".into());
    }
    Ok(())
}
fn path(id: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    let root = storage::root()?.join("tools");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(root.join(format!("{id}.json")))
}
fn revision(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn validate(value: Value) -> Result<Manifest, String> {
    if value.to_string().len() > 65536 {
        return Err("Tool manifest exceeds 64 KiB".into());
    }
    let manifest: Manifest =
        serde_json::from_value(value).map_err(|e| format!("Invalid tool manifest: {e}"))?;
    validate_id(&manifest.id)?;
    if manifest.schema_version != 1 || !matches!(manifest.target.as_str(), "windows" | "ssh") {
        return Err("Use schemaVersion 1 and target windows or ssh".into());
    }
    if manifest.version.split('.').count() != 3
        || manifest.version.len() > 40
        || manifest
            .version
            .split('.')
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Tool version must look like 1.0.0".into());
    }
    for (value, max, empty) in [
        (&manifest.title, 120, false),
        (&manifest.description, 2000, true),
        (&manifest.script, 32768, false),
    ] {
        if value.len() > max || (!empty && value.trim().is_empty()) || value.contains('\0') {
            return Err("Invalid tool text or script".into());
        }
    }
    let mut names = HashSet::new();
    if manifest.parameters.len() > 16 {
        return Err("At most 16 parameters are supported".into());
    }
    for p in &manifest.parameters {
        if p.name.is_empty()
            || p.name.len() > 40
            || !p.name.as_bytes()[0].is_ascii_alphabetic()
            || !p
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !names.insert(p.name.to_lowercase())
            || p.label.trim().is_empty()
            || p.label.len() > 100
            || p.label.contains('\0')
            || p.default_value
                .as_ref()
                .is_some_and(|v| v.len() > 2048 || v.contains('\0'))
        {
            return Err("Invalid or duplicate tool parameter".into());
        }
    }
    Ok(manifest)
}
pub fn read(id: &str) -> Result<(Manifest, String), String> {
    let path = path(id)?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err("Tool manifest must be a regular file up to 64 KiB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let manifest =
        validate(serde_json::from_slice(&bytes).map_err(|_| "Tool manifest is damaged")?)?;
    if manifest.id != id {
        return Err("Tool filename does not match its ID".into());
    }
    Ok((manifest, revision(&bytes)))
}
pub fn list() -> Result<Value, String> {
    let root = storage::root()?.join("tools");
    if !root.exists() {
        return Ok(json!([]));
    }
    let mut values = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let id = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("Invalid tool filename")?
            .to_owned();
        let (manifest, revision) = read(&id)?;
        values.push(json!({"manifest":manifest,"revision":revision}));
    }
    values.sort_by_key(|v| v["manifest"]["title"].as_str().unwrap_or("").to_lowercase());
    Ok(json!(values))
}
pub fn install(value: Value, expected: &str) -> Result<Value, String> {
    let manifest = validate(value)?;
    let destination = path(&manifest.id)?;
    let current = if destination.exists() {
        Some(read(&manifest.id)?.1)
    } else {
        None
    };
    if current.as_deref().unwrap_or("") != expected {
        return Err("Tool changed; review its current revision before replacing it".into());
    }
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Tool manifest exceeds 64 KiB".into());
    }
    let mut stage =
        tempfile::NamedTempFile::new_in(destination.parent().ok_or("Tool directory missing")?)
            .map_err(|e| e.to_string())?;
    stage.write_all(&bytes).map_err(|e| e.to_string())?;
    stage.as_file().sync_all().map_err(|e| e.to_string())?;
    if current.is_some() {
        if read(&manifest.id)?.1 != expected {
            return Err("Tool changed while installing".into());
        }
        stage
            .persist(destination)
            .map_err(|e| e.error.to_string())?;
    } else {
        stage
            .persist_noclobber(destination)
            .map_err(|e| e.error.to_string())?;
    }
    Ok(json!({"installed":manifest.id,"revision":revision(&bytes),"executed":false}))
}
fn command(manifest: &Manifest, arguments: &BTreeMap<String, String>) -> Result<String, String> {
    if arguments
        .keys()
        .any(|key| !manifest.parameters.iter().any(|p| p.name == *key))
    {
        return Err("Unknown tool parameter".into());
    }
    let mut script = String::new();
    for p in &manifest.parameters {
        let value = arguments
            .get(&p.name)
            .or(p.default_value.as_ref())
            .map(String::as_str)
            .unwrap_or("");
        if (p.required && value.is_empty()) || value.len() > 2048 || value.contains('\0') {
            return Err(format!("Enter a value for {}", p.label));
        }
        if manifest.target == "windows" {
            script.push_str(&format!(
                "$env:AETHER_TOOL_{} = '{}';\n",
                p.name,
                value.replace('\'', "''")
            ));
        } else {
            script.push_str(&format!(
                "AETHER_TOOL_{}='{}'; export AETHER_TOOL_{};\n",
                p.name,
                value.replace('\'', "'\\''"),
                p.name
            ));
        }
    }
    script.push_str(&manifest.script);
    Ok(script)
}
pub async fn run(
    id: &str,
    expected: &str,
    arguments: BTreeMap<String, String>,
    workspace: Workspace,
) -> Result<Value, String> {
    let (manifest, revision) = read(id)?;
    if revision != expected {
        return Err("The tool changed after review. Review it again".into());
    }
    if !matches!(
        (&workspace, manifest.target.as_str()),
        (Workspace::Local(_), "windows") | (Workspace::Ssh { .. }, "ssh")
    ) {
        return Err("Select the matching Windows or SSH workspace; tools never switch targets automatically".into());
    }
    ai_tools::execute_command(workspace, &command(&manifest, &arguments)?).await
}
pub struct Panel {
    tools: Value,
    error: Option<String>,
    selected: Option<(Manifest, String)>,
    arguments: BTreeMap<String, String>,
    output: String,
    result: Option<mpsc::Receiver<Result<Value, String>>>,
    cancel: CancellationToken,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            tools: json!([]),
            error: None,
            selected: None,
            arguments: BTreeMap::new(),
            output: String::new(),
            result: None,
            cancel: CancellationToken::new(),
        }
    }
}
impl Panel {
    pub fn is_busy(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|receiver| !receiver.is_closed())
    }
    pub fn stop(&self) {
        self.cancel.cancel();
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, runtime: &Connections, workspace: Workspace) {
        if let Some(receiver) = &mut self.result {
            if let Ok(result) = receiver.try_recv() {
                match result {
                    Ok(value) => {
                        self.output = serde_json::to_string_pretty(&value).unwrap_or_default()
                    }
                    Err(error) => self.error = Some(error),
                }
                self.result = None;
            }
        }
        if self.result.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
            ui.spinner();
            if ui.button("Stop tool").clicked() {
                self.stop();
            }
        }
        if ui.button("Refresh tools").clicked() {
            match list() {
                Ok(value) => self.tools = value,
                Err(error) => self.error = Some(error),
            }
        }
        ui.label("Ask the AI assistant to create a reusable tool, then review it here.");
        if let Some(tools) = self.tools.as_array() {
            for item in tools {
                if ui
                    .button(item["manifest"]["title"].as_str().unwrap_or("Tool"))
                    .clicked()
                {
                    let id = item["manifest"]["id"].as_str().unwrap_or("");
                    match read(id) {
                        Ok(value) => {
                            self.selected = Some(value);
                            self.arguments.clear();
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        }
        let mut execute = None;
        let mut remove = None;
        if let Some((manifest, revision)) = &self.selected {
            ui.heading(&manifest.title);
            ui.label(&manifest.description);
            ui.label(format!(
                "Target: {} · {}",
                manifest.target,
                workspace.label()
            ));
            egui::ScrollArea::vertical()
                .id_salt("tool-source")
                .max_height(180.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&manifest.script).monospace())
                            .selectable(true)
                            .wrap(),
                    );
                });
            let mut submit = false;
            for p in &manifest.parameters {
                ui.label(&p.label);
                let response = ui.text_edit_singleline(
                    self.arguments
                        .entry(p.name.clone())
                        .or_insert_with(|| p.default_value.clone().unwrap_or_default()),
                );
                submit |= crate::dialog::field_submit(ui, &response);
            }
            if ui
                .add_enabled(
                    self.result.is_none(),
                    egui::Button::new("Approve and run this tool"),
                )
                .clicked()
                || (submit && self.result.is_none())
            {
                execute = Some((manifest.id.clone(), revision.clone()));
            }
            if ui
                .add_enabled(self.result.is_none(), egui::Button::new("Remove tool"))
                .clicked()
            {
                remove = Some((manifest.id.clone(), revision.clone()));
            }
        }
        if let Some((id, expected)) = remove {
            let result = read(&id).and_then(|(_, current)| {
                if current != expected {
                    return Err("Tool changed; refresh before removing it".into());
                }
                std::fs::remove_file(path(&id)?).map_err(|e| e.to_string())
            });
            match result {
                Ok(()) => {
                    self.selected = None;
                    self.tools = list().unwrap_or(json!([]));
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((id, expected)) = execute {
            let (sender, receiver) = mpsc::channel(1);
            self.result = Some(receiver);
            self.cancel = CancellationToken::new();
            let cancel = self.cancel.clone();
            let args = self.arguments.clone();
            let ctx = ui.ctx().clone();
            runtime.spawn(async move{
                tracing::info!(target:"tools.audit",tool=%id,"Reviewed tool started");
                let result=tokio::select!{_ = cancel.cancelled()=>Err("Tool stopped".into()),result=run(&id,&expected,args,workspace)=>result};
                tracing::info!(target:"tools.audit",tool=%id,completed=result.is_ok(),"Reviewed tool completed or cancelled");
                let _=sender.send(result).await;ctx.request_repaint();
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        egui::ScrollArea::vertical()
            .id_salt("tool-output")
            .show(ui, |ui| {
                ui.add(egui::Label::new(&self.output).wrap().selectable(true));
            });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_reserved_devices_and_unknown_fields() {
        for id in ["../x", "con", "a--b", "-x", "A"] {
            assert!(validate_id(id).is_err());
        }
        let value = json!({"schemaVersion":1,"id":"good","title":"T","description":"","version":"1.0.0","target":"windows","script":"echo hi","surprise":true});
        assert!(validate(value).is_err());
    }
    #[test]
    fn parameters_are_quoted_data_not_source() {
        let m=validate(json!({"schemaVersion":1,"id":"good","title":"T","description":"","version":"1.0.0","target":"windows","script":"Write-Output $env:AETHER_TOOL_Name","parameters":[{"name":"Name","label":"Name"}]})).unwrap();
        let args = BTreeMap::from([("Name".into(), "'; Write-Output injected; '".into())]);
        assert!(command(&m, &args)
            .unwrap()
            .starts_with("$env:AETHER_TOOL_Name = '''; Write-Output injected; ''';"));
    }
}
