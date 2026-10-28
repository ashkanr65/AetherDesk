//! Preserve a bounded, redacted report before the private runtime is removed.
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Failure {
    pub pid: u32,
    pub display: u16,
    pub status: String,
    pub report: Option<PathBuf>,
}
impl Failure {
    pub fn message(&self) -> String {
        let location = self
            .report
            .as_ref()
            .map(|p| format!(" Diagnostics: {}", p.display()))
            .unwrap_or_default();
        format!("The local X11 display stopped ({}). Terminal and file transfer remain connected. Previous graphical applications may have closed. Use Restart X11 before launching new graphical applications.{}", self.status, location)
    }
}

pub fn sanitize(text: &str, secrets: &[&str]) -> String {
    let mut value = text.to_owned();
    for secret in secrets.iter().filter(|value| !value.is_empty()) {
        value = value.replace(secret, "<redacted>");
        value = value.replace(&secret.to_ascii_uppercase(), "<redacted>");
    }
    // MIT-MAGIC-COOKIE values may appear in either case in upstream errors.
    let cookies = regex::Regex::new(r"(?i)\b[0-9a-f]{32}\b").expect("static cookie pattern");
    cookies
        .replace_all(&value, "<redacted>")
        .lines()
        .map(|line| crate::logging::redact("x11_diagnostic", line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn preserve(
    directory: &Path,
    pid: u32,
    display: u16,
    status: String,
    secrets: &[&str],
) -> Failure {
    let mut failure = Failure {
        pid,
        display,
        status,
        report: None,
    };
    let write = || -> Result<PathBuf, String> {
        let root = crate::storage::root()?.join("logs");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        if let Ok(mut log) = std::fs::File::open(directory.join("server.log")) {
            let start = log
                .metadata()
                .map_err(|e| e.to_string())?
                .len()
                .saturating_sub(64 * 1024);
            log.seek(SeekFrom::Start(start))
                .map_err(|e| e.to_string())?;
            log.take(64 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
        }
        let path = root.join(format!(
            "x11-failure-{}-{pid}-{}.log",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let record = serde_json::json!({"ts":chrono::Utc::now().to_rfc3339(),"level":"warn","source":"local.x11",
            "target":"x11.runtime", "msg":"Bundled X11 server exited", "pid":pid,"display":display,
            "exit_status":failure.status, "display_configuration":crate::update_exit::display_description(),
            "renderer":"Shadow GDI; software OpenGL; authenticated multiwindow",
            "diagnostic":sanitize(&String::from_utf8_lossy(&bytes), secrets)});
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        writeln!(file, "{record}").map_err(|e| e.to_string())?;
        // Only our own flat report files are eligible for bounded retention.
        let mut reports = std::fs::read_dir(&root)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_type().is_ok_and(|t| t.is_file())
                    && e.file_name().to_string_lossy().starts_with("x11-failure-")
                    && e.file_name().to_string_lossy().ends_with(".log")
            })
            .collect::<Vec<_>>();
        reports.sort_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()));
        let excess = reports.len().saturating_sub(10);
        for old in reports.into_iter().take(excess) {
            let _ = std::fs::remove_file(old.path());
        }
        Ok(path)
    };
    match write() {
        Ok(path) => failure.report = Some(path),
        Err(error) => tracing::warn!(%error, "could not retain X11 failure diagnostics"),
    }
    tracing::warn!(pid=failure.pid, display=failure.display, exit_status=%failure.status,
        diagnostic_path=?failure.report, "local X11 display exited; retaining SSH connection");
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn x11_failure_reports_are_bounded_redacted_and_survive_runtime_cleanup() {
        let root = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(root.path(), || {
            let runtime = tempfile::tempdir().unwrap();
            let cookie = "1234567890abcdef1234567890abcdef";
            std::fs::write(
                runtime.path().join("server.log"),
                format!("{}\nFatal error cookie {cookie}\n", "x".repeat(100_000)),
            )
            .unwrap();
            let failure = preserve(
                runtime.path(),
                77,
                10,
                "exit code: 0xc0000005".into(),
                &[cookie],
            );
            let path = failure.report.unwrap();
            drop(runtime);
            let saved = std::fs::read_to_string(path).unwrap();
            assert!(!saved.contains(cookie));
            assert!(saved.contains("Fatal error"));
            assert!(saved.contains("0xc0000005"));
            assert!(saved.len() < 70 * 1024);
        });
    }
}
