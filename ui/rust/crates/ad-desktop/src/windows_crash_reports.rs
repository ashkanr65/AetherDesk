//! Import Windows crash/hang metadata on the next start, off the GUI thread.
//! Never changes Windows logging, enables dumps, or reads another app's reports.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Serialize, Deserialize)]
struct Process {
    pid: u32,
    path: String,
    started: i64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Run {
    process: Process,
    children: Vec<Process>,
    closed: Option<i64>,
}
struct Context {
    record: PathBuf,
    run: Run,
}
static CONTEXT: OnceLock<Arc<Mutex<Context>>> = OnceLock::new();

pub struct RunGuard {
    context: Option<Arc<Mutex<Context>>>,
}
impl RunGuard {
    pub fn finish(&self) {
        if let Some(context) = &self.context {
            if let Ok(mut context) = context.lock() {
                context.run.closed = Some(chrono::Utc::now().timestamp());
                let _ = save(&context.record, &context.run);
            }
        }
    }
}
fn save(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let root = path.parent().ok_or("Missing diagnostic directory")?;
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}
pub fn track_x11(pid: u32, executable: &Path) {
    if let Some(context) = CONTEXT.get() {
        if let Ok(mut context) = context.lock() {
            if context.run.children.len() < 32 {
                context.run.children.push(Process {
                    pid,
                    path: executable.to_string_lossy().into_owned(),
                    started: chrono::Utc::now().timestamp(),
                });
                let _ = save(&context.record, &context.run);
            }
        }
    }
}
pub fn begin(directory: &Path) -> RunGuard {
    #[cfg(not(windows))]
    {
        let _ = directory;
        return RunGuard { context: None };
    }
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return RunGuard { context: None };
        };
        let now = chrono::Utc::now().timestamp();
        let runs = directory.join("runs");
        let context = Arc::new(Mutex::new(Context {
            record: runs.join(format!("run-{}.json", uuid::Uuid::new_v4())),
            run: Run {
                process: Process {
                    pid: std::process::id(),
                    path: exe.to_string_lossy().into_owned(),
                    started: now,
                },
                children: Vec::new(),
                closed: None,
            },
        }));
        if let Ok(context) = context.lock() {
            let _ = save(&context.record, &context.run);
        }
        let _ = CONTEXT.set(context.clone());
        let directory = directory.to_path_buf();
        let current = context.lock().ok().map(|c| c.record.clone());
        let _=std::thread::Builder::new().name("aetherdesk-windows-reports".into()).spawn(move || {
            // Windows Error Reporting can publish just after the next launch.
            std::thread::sleep(std::time::Duration::from_secs(2));
            let result=(|| -> Result<usize,String> {
                let mut previous=Vec::new();
                if let Ok(entries)=std::fs::read_dir(&runs) {
                    for entry in entries.flatten() {
                        if Some(entry.path())==current || !entry.file_type().is_ok_and(|t|t.is_file()) {continue;}
                        if !entry.file_name().to_string_lossy().starts_with("run-") {continue;}
                        let Ok(meta)=entry.metadata() else {continue;};
                        if meta.len()>64*1024 {continue;}
                        if let Ok(bytes)=std::fs::read(entry.path()) {
                            if let Ok(run)=serde_json::from_slice::<Run>(&bytes) {
                                if (run.closed.is_none() || run.closed.is_some_and(|closed|closed>=now-3*86400)) && same_path(&run.process.path,&exe.to_string_lossy()) {previous.push(run);}
                            }
                        }
                    }
                }
                let events=read_windows()?;
                let count=import(&directory,&events,&previous,&exe.to_string_lossy(),chrono::Utc::now().timestamp())?;
                Ok(count)
            })();
            // Keep per-run metadata bounded. No core, crash or user logs are removed.
                if let Ok(entries)=std::fs::read_dir(&runs) {
                    let mut paths=entries.flatten().filter(|e|e.file_type().is_ok_and(|t|t.is_file())
                        && e.file_name().to_string_lossy().starts_with("run-") && e.file_name().to_string_lossy().ends_with(".json")).collect::<Vec<_>>();
                    paths.sort_by_key(|e|e.metadata().ok().and_then(|m|m.modified().ok()));
                    let excess=paths.len().saturating_sub(64);
                    for entry in paths.into_iter().take(excess) {if Some(entry.path())!=current {let _=std::fs::remove_file(entry.path());}}
                }
            match result {
                Ok(count) if count>0 => tracing::warn!(count,"Windows crash/hang reports from preceding runs were recovered into application logs"),
                Ok(_) => {},
                Err(error) => tracing::info!(%error,"Windows crash history could not be checked; startup continues"),
            }
        });
        RunGuard {
            context: Some(context),
        }
    }
}

#[derive(Clone, Debug)]
struct Report {
    provider: String,
    id: u32,
    record: String,
    time: String,
    utc: i64,
    data: BTreeMap<String, String>,
}
fn decode(text: &str) -> String {
    static ENTITIES: OnceLock<regex::Regex> = OnceLock::new();
    ENTITIES
        .get_or_init(|| {
            regex::Regex::new(r"&(?:#x([0-9a-fA-F]+)|#([0-9]+)|(amp|lt|gt|quot|apos));").unwrap()
        })
        .replace_all(text, |c: &regex::Captures<'_>| {
            if let Some(value) = c.get(1) {
                return u32::from_str_radix(value.as_str(), 16)
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{fffd}')
                    .to_string();
            }
            if let Some(value) = c.get(2) {
                return value
                    .as_str()
                    .parse()
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{fffd}')
                    .to_string();
            }
            match &c[3] {
                "amp" => "&",
                "lt" => "<",
                "gt" => ">",
                "quot" => "\"",
                _ => "'",
            }
            .into()
        })
        .into_owned()
}
fn parse(xml: &str) -> Option<Report> {
    if xml.len() > 128 * 1024 {
        return None;
    }
    let tag = |name: &str| {
        regex::Regex::new(&format!(r"<{name}(?:\s[^>]*)?>([^<]*)</{name}>"))
            .ok()?
            .captures(xml)
            .map(|c| decode(&c[1]))
    };
    let attribute = |tag: &str, name: &str| {
        regex::Regex::new(&format!(r#"<{tag}\b[^>]*\b{name}=['"]([^'"]*)['"]"#))
            .ok()?
            .captures(xml)
            .map(|c| decode(&c[1]))
    };
    let id = tag("EventID")?.parse().ok()?;
    if ![1000, 1001, 1002].contains(&id) {
        return None;
    }
    let time = attribute("TimeCreated", "SystemTime")?;
    let utc = chrono::DateTime::parse_from_rfc3339(&time)
        .ok()?
        .timestamp();
    static DATA: OnceLock<regex::Regex> = OnceLock::new();
    let data = DATA
        .get_or_init(|| {
            regex::Regex::new(r#"(?s)<Data\s+Name=['"]([^'"]+)['"]>(.*?)</Data>"#).unwrap()
        })
        .captures_iter(xml)
        .map(|c| (c[1].to_ascii_lowercase(), decode(&c[2])))
        .collect();
    Some(Report {
        provider: attribute("Provider", "Name")?,
        id,
        record: tag("EventRecordID")?,
        time,
        utc,
        data,
    })
}
fn same_path(left: &str, right: &str) -> bool {
    let normalize = |path: &str| {
        path.trim()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}
fn pid(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or_else(|| value.parse().ok(), |v| u32::from_str_radix(v, 16).ok())
}
fn owns(report: &Report, runs: &[Run], exe: &str, now: i64) -> bool {
    if report.utc < now - 3 * 86400 || report.utc > now + 2 {
        return false;
    }
    let Some(path) = report
        .data
        .get("apppath")
        .or_else(|| report.data.get("applicationpath"))
        .or_else(|| report.data.get("exefilename"))
    else {
        return false;
    };
    let x11 = Path::new(exe).parent().map(|p| p.join("x11/vcxsrv.exe"));
    if !same_path(path, exe) && !x11.is_some_and(|p| same_path(path, &p.to_string_lossy())) {
        return false;
    }
    let process = report.data.get("processid").and_then(|v| pid(v));
    let mut known = false;
    for run in runs {
        for item in std::iter::once(&run.process).chain(run.children.iter()) {
            if Some(item.pid) == process && same_path(path, &item.path) {
                known = true;
                if report.utc >= item.started.saturating_sub(1)
                    && report.utc <= run.closed.unwrap_or(now) + 2
                {
                    return true;
                }
            }
        }
    }
    // Supports the first upgraded launch, before previous builds had run markers.
    !known
}
fn import(
    directory: &Path,
    reports: &[Report],
    runs: &[Run],
    exe: &str,
    now: i64,
) -> Result<usize, String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut count = 0;
    let mut ids = HashSet::new();
    let owned_reports: HashSet<String> = reports
        .iter()
        .filter(|r| owns(r, runs, exe, now))
        .filter_map(|r| {
            r.data
                .get("integratorreportid")
                .or_else(|| r.data.get("reportid"))
        })
        .filter(|id| !id.is_empty())
        .map(|id| id.to_ascii_lowercase())
        .collect();
    for report in reports.iter().filter(|r| {
        owns(r, runs, exe, now)
            || (r.id == 1001
                && r.provider.eq_ignore_ascii_case("Windows Error Reporting")
                && r.utc >= now - 3 * 86400
                && r.utc <= now + 2
                && r.data
                    .get("reportid")
                    .is_some_and(|id| owned_reports.contains(&id.to_ascii_lowercase())))
    }) {
        let key = format!("{}|{}|{}", report.provider, report.record, report.time);
        let hash = ring::digest::digest(&ring::digest::SHA256, key.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if !ids.insert(hash.clone()) {
            continue;
        }
        let path = directory.join(format!("windows-event-{hash}.log"));
        if path.exists() {
            continue;
        }
        let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
        let allowed = [
            "appname",
            "apppath",
            "applicationpath",
            "exefilename",
            "appversion",
            "processid",
            "modulename",
            "modulepath",
            "moduleversion",
            "exceptioncode",
            "faultingoffset",
            "hangtype",
            "reportid",
            "integratorreportid",
            "eventname",
        ];
        let fields = report
            .data
            .iter()
            .filter(|(name, _)| allowed.contains(&name.as_str()))
            .map(|(k, v)| (k.clone(), crate::logging::redact("windows_metadata", v)))
            .collect::<BTreeMap<_, _>>();
        let record = serde_json::json!({"ts":report.time,"level":"error","source":"windows.eventlog","target":report.provider,
            "msg":if report.id==1002 {"Windows reported an application hang"} else {"Windows reported an application failure"},
            "event_id":report.id,"event_record_id":report.record,"imported_at":chrono::Utc::now().to_rfc3339(),"details":fields});
        use std::io::Write;
        writeln!(file, "{record}").map_err(|e| e.to_string())?;
        file.as_file().sync_data().map_err(|e| e.to_string())?;
        match file.persist_noclobber(&path) {
            Ok(_) => count += 1,
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.error.to_string()),
        }
    }
    // Deterministic filenames also deduplicate simultaneous startup scans.
    // Reports beyond the 72-hour query window are eligible for removal.
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten().filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("windows-event-")
                && e.file_name().to_string_lossy().ends_with(".log")
        }) {
            if entry.file_type().is_ok_and(|t| t.is_file())
                && entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .is_some_and(|t| {
                        std::time::SystemTime::now()
                            .duration_since(t)
                            .is_ok_and(|age| age.as_secs() > 7 * 86400)
                    })
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(count)
}

#[cfg(windows)]
fn read_windows() -> Result<Vec<Report>, String> {
    use windows_sys::Win32::System::EventLog::*;
    struct Handle(EVT_HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                EvtClose(self.0);
            }
        }
    }
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let channel = wide("Application");
    let query=wide("*[System[(EventID=1000 or EventID=1001 or EventID=1002) and TimeCreated[timediff(@SystemTime) <= 259200000]]]");
    let result = unsafe {
        EvtQuery(
            0,
            channel.as_ptr(),
            query.as_ptr(),
            EvtQueryChannelPath | EvtQueryReverseDirection,
        )
    };
    if result == 0 {
        return Err(format!(
            "Windows event query unavailable ({})",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        ));
    }
    let result = Handle(result);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut reports = Vec::new();
    let mut scanned = 0;
    while scanned < 256 && std::time::Instant::now() < deadline {
        let mut handles = [0; 8];
        let mut returned = 0;
        let success = unsafe { EvtNext(result.0, 8, handles.as_mut_ptr(), 100, 0, &mut returned) };
        if success == 0 {
            break;
        }
        for handle in handles.into_iter().take(returned as usize) {
            let event = Handle(handle);
            scanned += 1;
            let (mut used, mut properties) = (0, 0);
            unsafe {
                EvtRender(
                    0,
                    event.0,
                    EvtRenderEventXml,
                    0,
                    std::ptr::null_mut(),
                    &mut used,
                    &mut properties,
                );
            }
            if used == 0 || used > 128 * 1024 {
                continue;
            }
            let mut buffer = vec![0u16; (used as usize + 1) / 2];
            if unsafe {
                EvtRender(
                    0,
                    event.0,
                    EvtRenderEventXml,
                    used,
                    buffer.as_mut_ptr().cast(),
                    &mut used,
                    &mut properties,
                )
            } == 0
            {
                continue;
            }
            let end = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
            if let Some(report) = parse(&String::from_utf16_lossy(&buffer[..end])) {
                reports.push(report);
            }
        }
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(path: &str) -> Report {
        parse(&format!(r#"<Event><System><Provider Name='Application Error'/><EventID>1000</EventID><TimeCreated SystemTime='2026-10-27T12:00:42Z'/><EventRecordID>123</EventRecordID></System><EventData><Data Name='AppName'>AetherDesk.exe</Data><Data Name='AppPath'>{path}</Data><Data Name='ProcessId'>0x2a</Data><Data Name='ExceptionCode'>c0000005</Data><Data Name='CommandLine'>private document content</Data></EventData></Event>"#)).unwrap()
    }
    #[test]
    fn windows_reports_match_previous_pid_path_and_time_not_just_the_filename() {
        let report = fixture(r"C:\Apps\AetherDesk.exe");
        let now = report.utc + 60;
        let run = Run {
            process: Process {
                pid: 42,
                path: r"C:\Apps\AetherDesk.exe".into(),
                started: report.utc - 3600,
            },
            children: Vec::new(),
            closed: None,
        };
        assert!(owns(
            &report,
            &[run.clone()],
            r"C:\Apps\AetherDesk.exe",
            now
        ));
        assert!(!owns(
            &fixture(r"C:\Other\AetherDesk.exe"),
            &[run.clone()],
            r"C:\Apps\AetherDesk.exe",
            now
        ));
        let mut reused = run.clone();
        reused.process.started = now;
        assert!(!owns(&report, &[reused], r"C:\Apps\AetherDesk.exe", now));
        assert!(!owns(
            &report,
            &[run],
            r"C:\Apps\AetherDesk.exe",
            now + 4 * 86400
        ));
    }
    #[test]
    fn windows_crash_import_deduplicates_and_excludes_unrelated_payloads() {
        let root = tempfile::tempdir().unwrap();
        let report = fixture(r"C:\Apps\AetherDesk.exe");
        assert_eq!(
            import(
                root.path(),
                &[report.clone(), report.clone()],
                &[],
                r"C:\Apps\AetherDesk.exe",
                report.utc + 60
            )
            .unwrap(),
            1
        );
        assert_eq!(
            import(
                root.path(),
                &[report.clone()],
                &[],
                r"C:\Apps\AetherDesk.exe",
                report.utc + 60
            )
            .unwrap(),
            0
        );
        let files = std::fs::read_dir(root.path()).unwrap().collect::<Vec<_>>();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        assert!(text.contains("c0000005"));
        assert!(!text.contains("private document content"));
        assert_eq!(decode("x&amp;y&#x20;&#65;"), "x&y A");
    }
    #[test]
    fn owned_x11_crashes_and_correlated_wer_reports_are_imported_but_unrelated_reports_are_not() {
        let mut application = fixture(r"C:\Apps\AetherDesk.exe");
        application
            .data
            .insert("integratorreportid".into(), "owned-report".into());
        let mut wer = application.clone();
        wer.id = 1001;
        wer.record = "124".into();
        wer.provider = "Windows Error Reporting".into();
        wer.data.clear();
        wer.data.insert("reportid".into(), "owned-report".into());
        let mut other = wer.clone();
        other.record = "125".into();
        other
            .data
            .insert("reportid".into(), "another-report".into());
        let mut hang = application.clone();
        hang.id = 1002;
        hang.record = "126".into();
        hang.provider = "Application Hang".into();
        let path = hang.data.remove("apppath").unwrap();
        hang.data.insert("exefilename".into(), path);
        let x11 = fixture(r"C:\Apps\x11\vcxsrv.exe");
        let run = Run {
            process: Process {
                pid: 99,
                path: r"C:\Apps\AetherDesk.exe".into(),
                started: x11.utc - 60,
            },
            children: vec![Process {
                pid: 42,
                path: r"C:\Apps\x11\vcxsrv.exe".into(),
                started: x11.utc - 50,
            }],
            closed: Some(x11.utc + 30),
        };
        assert!(owns(&x11, &[run], r"C:\Apps\AetherDesk.exe", x11.utc + 60));
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            import(
                root.path(),
                &[application.clone(), wer, other, hang],
                &[],
                r"C:\Apps\AetherDesk.exe",
                application.utc + 60
            )
            .unwrap(),
            3
        );
    }
    #[test]
    #[cfg(windows)]
    fn native_windows_report_query_is_bounded_and_read_only() {
        let start = std::time::Instant::now();
        let result = read_windows();
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        if let Ok(reports) = result {
            assert!(reports.len() <= 256);
        }
    }
}
