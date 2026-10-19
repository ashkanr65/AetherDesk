//! Application log capture for the shipped client.
//!
//! The Flutter host supplies the log directory via its application log path.
//! Rust owns a separate core-PID.log file and bounded background writer.
//! The Logs panel merges producer files without competing rotation handles.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatEvent};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;

const MAX_BYTES: u64 = 10 * 1024 * 1024;
const GENERATIONS: usize = 5;

/// Installs the JSONL file subscriber once. Unknown level strings fall back
/// to `info`, matching the host agent's default.
pub fn init(path: &Path) {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let level = std::env::var("AETHERDESK_LOG_LEVEL").unwrap_or_else(|_| "info".into());
        let filter = EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new("info"));
        let path = path.with_file_name(format!("core-{}.log", std::process::id()));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let shared = AsyncFile::open(path);
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .event_format(JsonLine)
            .with_writer(move || shared.clone())
            .try_init();
    });
}

/// Logging must never block SSH/video on disk I/O. Overload is bounded and
/// reported by the worker when it next drains the queue.
#[derive(Clone)]
struct AsyncFile {
    sender: mpsc::SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
}
impl AsyncFile {
    fn open(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(256);
        let dropped = Arc::new(AtomicU64::new(0));
        let counter = dropped.clone();
        std::thread::spawn(move || {
            let file = SharedFile::open(path);
            while let Ok(record) = receiver.recv() {
                let count = counter.swap(0, Ordering::Relaxed);
                if count != 0 {
                    let warning = serde_json::json!({"ts": chrono::Utc::now().to_rfc3339(),
                        "level":"warn", "source":"core.rust", "msg":format!("Dropped {count} log records while the log queue was full")});
                    file.append(format!("{warning}\n").as_bytes());
                }
                file.append(&record);
            }
        });
        Self { sender, dropped }
    }
}
impl Write for AsyncFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.sender.try_send(bytes.to_vec()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One shared appender. Every write is one complete JSONL line produced by
/// [`JsonLine`], so the mutex serializes whole records even while the video
/// and SSH threads are logging concurrently.
#[derive(Clone)]
struct SharedFile(Arc<Mutex<SharedState>>);

struct SharedState {
    path: PathBuf,
    file: Option<File>,
    written: u64,
}

impl SharedFile {
    fn open(path: PathBuf) -> Self {
        Self(Arc::new(Mutex::new(SharedState {
            path,
            file: None,
            written: 0,
        })))
    }

    fn append(&self, bytes: &[u8]) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if state.file.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.path);
            match file {
                Ok(file) => {
                    state.written = file.metadata().map(|m| m.len()).unwrap_or(0);
                    state.file = Some(file);
                }
                Err(_) => return,
            }
        }
        if state.written.saturating_add(bytes.len() as u64) > MAX_BYTES {
            state.file.take(); // Close before rotating on Windows.
            rotate(&state.path);
            state.written = 0;
            state.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.path)
                .ok();
            if state.file.is_none() {
                return;
            }
        }
        let written = bytes.len() as u64;
        let accepted = match state.file.as_mut() {
            Some(file) => file.write_all(bytes).is_ok(),
            None => false,
        };
        if accepted {
            state.written += written;
            if let Some(file) = state.file.as_mut() {
                let _ = file.flush();
            }
        }
    }
}

fn rotate(path: &Path) {
    let oldest = PathBuf::from(format!("{}.{}", path.display(), GENERATIONS));
    let _ = std::fs::remove_file(&oldest);
    for index in (1..GENERATIONS).rev() {
        let source = PathBuf::from(format!("{}.{}", path.display(), index));
        if source.exists() {
            let _ = std::fs::rename(&source, format!("{}.{}", path.display(), index + 1));
        }
    }
    let _ = std::fs::rename(path, format!("{}.1", path.display()));
}

impl Write for SharedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.append(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes each event as one complete line matching the Dart `AppLogRecord`
/// schema: `{"ts","level","source","target","msg",…}`.
struct JsonLine;

impl<S, N> FormatEvent<S, N> for JsonLine
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let mut fields = serde_json::Map::new();
        event.record(&mut JsonFields(&mut fields));
        let mut record = serde_json::Map::new();
        record.insert(
            "ts".into(),
            serde_json::Value::String(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
        );
        record.insert(
            "level".into(),
            serde_json::Value::String(metadata.level().as_str().to_ascii_lowercase()),
        );
        record.insert("source".into(), "core.rust".into());
        record.insert("target".into(), metadata.target().into());
        for (key, value) in fields {
            if key == "message" {
                record.insert("msg".into(), value);
            } else {
                record.insert(key, value);
            }
        }
        let mut line = serde_json::Value::Object(record).to_string();
        line.push('\n');
        // One write_str call: the appender sees the whole record at once.
        std::fmt::Write::write_str(&mut writer, &line)
    }
}

/// Collects event fields as JSON values without quoting strings twice.
struct JsonFields<'a>(&'a mut serde_json::Map<String, serde_json::Value>);

impl Visit for JsonFields<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        let unquoted = text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(&text)
            .to_string();
        self.record_text(field.name(), unquoted);
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_text(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), value.into());
    }
}

impl JsonFields<'_> {
    fn record_text(&mut self, name: &str, value: String) {
        self.0.insert(
            name.replace('.', "_"),
            serde_json::Value::String(redact(name, &value)),
        );
    }
}

pub(crate) fn redact(name: &str, value: &str) -> String {
    let field = name.to_ascii_lowercase();
    let lower = value.to_ascii_lowercase();
    let keys = [
        "password",
        "passphrase",
        "secret",
        "token",
        "api_key",
        "apikey",
        "api-key",
        "authorization",
        "private_key",
    ];
    if keys.iter().any(|key| field.contains(key))
        || matches!(
            field.as_str(),
            "command"
                | "cmd"
                | "script"
                | "clipboard"
                | "payload"
                | "arguments"
                | "stdout"
                | "stderr"
        )
        || lower.contains("aetherdesk_agent_v1")
        || lower.contains("private key-----")
        || lower.contains("bearer ")
        || lower.contains("basic ")
        || keys.iter().any(|key| {
            lower.contains(&format!("{key}="))
                || lower.contains(&format!("{key}:"))
                || lower.contains(&format!("\"{key}\""))
        })
    {
        return "<redacted sensitive detail>".into();
    }
    value.chars().take(16384).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensitive_fields_and_embedded_tokens_never_reach_disk() {
        for (name, value) in [
            ("password", "hunter2"),
            ("message", "AETHERDESK_AGENT_V1 44 secret"),
            ("error", "authorization: Bearer secret"),
            ("msg", "{\"token\":\"secret\"}"),
        ] {
            assert!(!redact(name, value).contains(value));
        }
        assert_eq!(redact("message", "stream stalled"), "stream stalled");
    }
    #[test]
    fn file_writer_rotates_and_keeps_complete_records() {
        let dir = std::env::temp_dir().join(format!("aetherdesk-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("core.log");
        let file = SharedFile::open(path.clone());
        file.append(b"{\"msg\":\"first\"}\n");
        file.0.lock().unwrap().written = MAX_BYTES;
        file.append(b"{\"msg\":\"second\"}\n");
        assert!(std::fs::read_to_string(&path).unwrap().contains("second"));
        assert!(std::fs::read_to_string(dir.join("core.log.1"))
            .unwrap()
            .contains("first"));
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
