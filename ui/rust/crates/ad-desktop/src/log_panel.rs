//! Bounded structured logs. Disk, SSH and export operations stay off the UI thread.
use eframe::egui;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
const MAX_READ: usize = 2 * 1024 * 1024;
const MAX_RECORDS: usize = 4000;
const LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];
type Host = (String, Arc<ad_ssh::SshSession>, uuid::Uuid);
#[derive(Clone, Debug)]
struct Record {
    timestamp: String,
    order: i64,
    level: String,
    source: String,
    target: String,
    message: String,
    detail: String,
}
impl Record {
    fn parse(line: &str, source: &str) -> Self {
        if let Ok(serde_json::Value::Object(mut value)) = serde_json::from_str(line) {
            let mut take = |key: &str| {
                value
                    .remove(key)
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            };
            let timestamp = take("ts");
            let level = normalize_level(&take("level"));
            let named_source = crate::logging::redact("source", &take("source"));
            let target = crate::logging::redact("target", &take("target"));
            let message = take("msg");
            let message = if message.is_empty() {
                take("message")
            } else {
                message
            };
            let detail = value
                .into_iter()
                .map(|(k, v)| format!("{k}={}", crate::logging::redact(&k, &v.to_string())))
                .collect::<Vec<_>>()
                .join(" ");
            Self {
                order: chrono::DateTime::parse_from_rfc3339(&timestamp)
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0),
                timestamp,
                level,
                source: if named_source.is_empty() {
                    source.into()
                } else {
                    named_source
                },
                target,
                message: crate::logging::redact("message", &message),
                detail,
            }
        } else {
            let words = line.split_whitespace().collect::<Vec<_>>();
            let timestamp = words
                .first()
                .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
                .copied()
                .unwrap_or_default()
                .to_owned();
            let severity = words.iter().take(3).find(|s| {
                matches!(
                    s.to_ascii_lowercase().as_str(),
                    "trace" | "debug" | "info" | "warn" | "warning" | "error"
                )
            });
            Self {
                order: chrono::DateTime::parse_from_rfc3339(&timestamp)
                    .map(|t| t.timestamp_millis())
                    .unwrap_or(0),
                timestamp,
                level: normalize_level(severity.copied().unwrap_or("info")),
                source: source.into(),
                target: String::new(),
                message: crate::logging::redact("message", line),
                detail: String::new(),
            }
        }
    }
    fn display(&self) -> String {
        crate::logging::redact(
            "message",
            &format!(
                "{} {} [{}{}] {}{}",
                self.timestamp,
                self.level.to_uppercase(),
                self.source,
                if self.target.is_empty() {
                    String::new()
                } else {
                    format!("/{}", self.target)
                },
                self.message,
                if self.detail.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", self.detail)
                }
            ),
        )
    }
}
fn normalize_level(level: &str) -> String {
    match level.to_ascii_lowercase().as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "warn" | "warning" => "warn",
        "error" => "error",
        _ => "info",
    }
    .into()
}
fn is_log(name: &str) -> bool {
    name.ends_with(".log")
        || name
            .rsplit_once(".log.")
            .is_some_and(|(_, n)| n.parse::<u32>().is_ok())
}
fn read_local(directory: &Path) -> Result<Vec<Record>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_ok_and(|t| t.is_file()) && is_log(&e.file_name().to_string_lossy())
        })
        .map(|e| e.path())
        .collect::<Vec<_>>();
    paths.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let mut remaining = MAX_READ;
    let mut records = Vec::new();
    for path in paths.iter().rev().take(64) {
        if remaining == 0 {
            break;
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            continue;
        };
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        let amount = remaining.min(256 * 1024) as u64;
        let offset = size.saturating_sub(amount);
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(amount)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        remaining = remaining.saturating_sub(bytes.len());
        let text = String::from_utf8_lossy(&bytes);
        let text = if offset > 0 {
            text.split_once('\n').map(|(_, s)| s).unwrap_or("")
        } else {
            &text
        };
        records.extend(
            text.lines()
                .filter(|s| !s.trim().is_empty())
                .map(|s| Record::parse(s, "core.rust")),
        );
    }
    records.sort_by_key(|r| r.order);
    if records.len() > MAX_RECORDS {
        records.drain(..records.len() - MAX_RECORDS);
    }
    Ok(records)
}
fn visible(
    record: &Record,
    mode: usize,
    query: &str,
    case: bool,
    levels: &[bool; 5],
    source: &str,
) -> bool {
    let index = LEVELS.iter().position(|s| *s == record.level).unwrap_or(2);
    if !levels[index] || (!source.is_empty() && source != record.source) {
        return false;
    }
    if mode == 2 && !(record.target == "ai.audit" || record.target.starts_with("tools.audit")) {
        return false;
    }
    if mode == 3 && index < 3 {
        return false;
    }
    let text = record.display();
    if case {
        text.contains(query)
    } else {
        text.to_lowercase().contains(&query.to_lowercase())
    }
}
enum Reply {
    Records(Result<Vec<Record>, String>),
    Operation(Result<String, String>),
}
pub struct Panel {
    mode: usize,
    query: String,
    case: bool,
    levels: [bool; 5],
    source: String,
    follow: bool,
    records: Vec<Record>,
    error: Option<String>,
    last: Instant,
    receiver: Option<mpsc::Receiver<Reply>>,
    export_path: String,
    host_id: Option<uuid::Uuid>,
    host_connection: String,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            mode: 0,
            query: String::new(),
            case: false,
            levels: [true; 5],
            source: String::new(),
            follow: true,
            records: Vec::new(),
            error: None,
            last: Instant::now() - Duration::from_secs(10),
            receiver: None,
            export_path: String::new(),
            host_id: None,
            host_connection: String::new(),
        }
    }
}
impl Panel {
    fn invalidate(&mut self) {
        self.receiver = None;
        self.records.clear();
        self.source.clear();
        self.error = None;
        self.last = Instant::now() - Duration::from_secs(10);
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        runtime: &crate::connection::Connections,
        hosts: Vec<Host>,
    ) {
        if self.host_id.is_none() && !hosts.is_empty() {
            self.host_id = Some(hosts[0].2);
        }
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            for (index, label) in ["Application", "Host", "Audit", "Problems"]
                .into_iter()
                .enumerate()
            {
                if ui.selectable_label(self.mode == index, label).clicked() {
                    self.mode = index;
                    self.invalidate();
                    refresh = true;
                }
            }
        });
        if self.mode == 1 {
            egui::ComboBox::from_id_salt("host-log-source")
                .selected_text(
                    hosts
                        .iter()
                        .find(|h| Some(h.2) == self.host_id)
                        .map(|h| h.0.as_str())
                        .unwrap_or("Disconnected host"),
                )
                .show_ui(ui, |ui| {
                    for (name, _, id) in &hosts {
                        if ui
                            .selectable_value(&mut self.host_id, Some(*id), name)
                            .changed()
                        {
                            refresh = true;
                        }
                    }
                });
            let connection = hosts
                .iter()
                .find(|h| Some(h.2) == self.host_id)
                .map(|h| format!("{:?}/{}", h.1.connection_id(), h.2))
                .unwrap_or_default();
            if connection != self.host_connection || refresh {
                self.host_connection = connection;
                self.invalidate();
                refresh = true;
            }
        }
        if let Some(receiver) = &mut self.receiver {
            match receiver.try_recv() {
                Ok(reply) => {
                    self.receiver = None;
                    match reply {
                        Reply::Records(Ok(records)) => {
                            self.records = records;
                            self.error = None;
                        }
                        Reply::Operation(Ok(message)) => self.error = Some(message),
                        Reply::Records(Err(e)) | Reply::Operation(Err(e)) => self.error = Some(e),
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.error = Some("Log operation did not complete".into());
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
        }
        ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search messages"));
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.case, "Match case");
            for (i, level) in LEVELS.iter().enumerate() {
                ui.checkbox(&mut self.levels[i], *level);
            }
        });
        let mut sources = self
            .records
            .iter()
            .map(|r| r.source.clone())
            .collect::<Vec<_>>();
        sources.sort();
        sources.dedup();
        egui::ComboBox::from_label("Source")
            .selected_text(if self.source.is_empty() {
                "All sources"
            } else {
                &self.source
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.source, String::new(), "All sources");
                for source in sources {
                    ui.selectable_value(&mut self.source, source.clone(), source);
                }
            });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.follow, "Follow");
            refresh |= ui.button("Refresh").clicked();
        });
        let interval = if self.mode == 1 {
            Duration::from_secs(8)
        } else {
            Duration::from_secs(2)
        };
        if self.receiver.is_none() && (refresh || self.follow && self.last.elapsed() >= interval) {
            self.last = Instant::now();
            let (sender, receiver) = mpsc::channel(1);
            self.receiver = Some(receiver);
            let host = hosts.iter().find(|h| Some(h.2) == self.host_id).cloned();
            let remote = self.mode == 1;
            let ctx = ui.ctx().clone();
            runtime.spawn(async move {
                let result=if remote {match host {
                    Some((_,session,id))=>session.execute_command_output(&format!("tail -c 262144 \"$HOME/.aetherdesk/sessions/{id}/agent.log\" 2>/dev/null"),Duration::from_secs(8)).await.map_err(|e|e.to_string()).and_then(|o|if o.exit_status==0 {Ok(String::from_utf8_lossy(&o.stdout).lines().map(|line|Record::parse(line,"host.agent")).take(MAX_RECORDS).collect())}else{Err("Host log is not available for this desktop session".into())}),
                    None=>Err("Connect a remote desktop to read its host log".into()),
                }}else{tokio::task::spawn_blocking(||read_local(&crate::storage::root()?.join("logs"))).await.map_err(|e|e.to_string()).and_then(|r|r)};
                let _=sender.send(Reply::Records(result)).await;ctx.request_repaint();
            });
        }
        if self.follow || self.receiver.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        let records = self
            .records
            .iter()
            .filter(|r| {
                visible(
                    r,
                    self.mode,
                    &self.query,
                    self.case,
                    &self.levels,
                    &self.source,
                )
            })
            .collect::<Vec<_>>();
        ui.label(format!(
            "{} visible / {} loaded",
            records.len(),
            self.records.len()
        ));
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 6.0;
        egui::ScrollArea::both()
            .id_salt("structured-logs")
            .stick_to_bottom(self.follow)
            .max_height((ui.available_height() - 130.0).max(50.0))
            .show_rows(ui, row_height, records.len(), |ui, range| {
                for index in range {
                    let record = records[index];
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(record.display().replace('\n', " ⏎ ")).monospace(),
                        )
                        .selectable(true),
                    );
                }
            });
        let filtered = records
            .iter()
            .map(|r| r.display())
            .collect::<Vec<_>>()
            .join("\n");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy visible").clicked() {
                ui.ctx().copy_text(filtered.clone());
            }
            if ui
                .add_enabled(
                    self.receiver.is_none(),
                    egui::Button::new("Open log folder"),
                )
                .clicked()
            {
                self.operation(runtime, ui.ctx(), None, String::new());
            }
        });
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.export_path)
                .hint_text("Export to a new local file"),
        );
        let enter = crate::dialog::field_submit(ui, &response);
        if ui
            .add_enabled(
                self.receiver.is_none(),
                egui::Button::new("Export displayed logs"),
            )
            .clicked()
            || (enter && self.receiver.is_none())
        {
            self.operation(
                runtime,
                ui.ctx(),
                Some(PathBuf::from(&self.export_path)),
                filtered,
            );
        }
    }
    fn operation(
        &mut self,
        runtime: &crate::connection::Connections,
        ctx: &egui::Context,
        path: Option<PathBuf>,
        text: String,
    ) {
        let (sender, receiver) = mpsc::channel(1);
        self.receiver = Some(receiver);
        let ctx = ctx.clone();
        runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
                if let Some(path) = path {
                    let mut f = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(path)
                        .map_err(|e| e.to_string())?;
                    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
                    Ok("Displayed logs exported".into())
                } else {
                    let directory = crate::storage::root()?.join("logs");
                    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        std::process::Command::new("explorer.exe")
                            .arg(directory)
                            .creation_flags(0x08000000)
                            .spawn()
                            .map_err(|e| e.to_string())?;
                        Ok("Log folder opened".into())
                    }
                    #[cfg(not(windows))]
                    {
                        Err(format!("Log folder: {}", directory.display()))
                    }
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = sender.send(Reply::Operation(result)).await;
            ctx.request_repaint();
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_filters_do_not_treat_error_words_as_severity_or_lose_rotations() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("core-1.log.1"),
            "{\"ts\":\"2026-01-01T00:00:00Z\",\"level\":\"warn\",\"msg\":\"Older warning\"}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("core-1.log"),"{\"ts\":\"2026-01-01T00:00:01Z\",\"level\":\"info\",\"msg\":\"error recovered\",\"token\":\"private-token\"}\n{\"level\":\"error\",\"target\":\"ai.audit\",\"msg\":\"Authorization: Bearer hidden\"}\n").unwrap();
        std::fs::write(dir.path().join("not-a-log.txt"), "must not display").unwrap();
        let records = read_local(dir.path()).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .filter(|r| visible(r, 3, "", false, &[true; 5], ""))
                .count(),
            2
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| visible(r, 2, "", false, &[true; 5], ""))
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| visible(r, 0, "older", true, &[true; 5], ""))
                .count(),
            0
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| visible(r, 0, "older", false, &[true; 5], "core.rust"))
                .count(),
            1
        );
        let text = records
            .iter()
            .map(Record::display)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("hidden"));
        assert!(!text.contains("private-token"));
    }
    #[test]
    fn truncated_record_is_dropped_and_reading_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("core.log"),
            format!(
                "{}\n{{\"level\":\"error\",\"msg\":\"complete\"}}\n",
                "x".repeat(MAX_READ + 100)
            ),
        )
        .unwrap();
        let records = read_local(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message, "complete");
    }
}
