use ad_ssh::{FileProgress, RemoteFileRequest, SshSession};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub enum Workspace {
    Local(PathBuf),
    Ssh {
        session: Arc<SshSession>,
        root: String,
        label: String,
        id: String,
    },
}
/// Pretty paths are for presentation only. Scope checks retain canonical paths.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    if let Some(drive) = text.strip_prefix(r"\\?\") {
        let bytes = drive.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\'
        {
            return drive.to_owned();
        }
    }
    text.into_owned()
}
impl Workspace {
    pub fn label(&self) -> String {
        match self {
            Self::Local(p) => display_path(p),
            Self::Ssh { label, root, .. } => format!("{label}:{root}"),
        }
    }
    pub fn id(&self) -> String {
        match self {
            Self::Local(p) => format!("local:{}", p.display()),
            Self::Ssh { id, root, .. } => format!("ssh:{id}:{root}"),
        }
    }
}
pub fn schemas() -> Value {
    let mut schemas = json!([
        {"type":"function","function":{"name":"list_tools","description":"List reusable Tools menu manifests and their revisions","parameters":{"type":"object","properties":{},"additionalProperties":false}}},
    {"type":"function","function":{"name":"install_tool","description":"Install a reusable Tools menu manifest without executing it. Schema: schemaVersion=1, id (lowercase hyphenated), title, description, version (1.0.0), target (windows or ssh), script, parameters [{name,label,required,defaultValue}]. Scripts read parameters from AETHER_TOOL_Name environment variables. expected_revision must be empty for new tools or match list_tools for replacements.","parameters":{"type":"object","properties":{"manifest":{"type":"object"},"expected_revision":{"type":"string"}},"required":["manifest","expected_revision"],"additionalProperties":false}}},
    {"type":"function","function":{"name":"run_tool","description":"Run a previously reviewed tool revision in the current workspace","parameters":{"type":"object","properties":{"id":{"type":"string"},"expected_revision":{"type":"string"},"arguments":{"type":"object"}},"required":["id","expected_revision","arguments"],"additionalProperties":false}}},
    {"type":"function","function":{"name":"list_files","description":"List a directory within the selected workspace","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"read_file","description":"Read a UTF-8 file and its revision within the selected workspace","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"write_file","description":"Save a UTF-8 file only if expected_revision matches the read_file result. Use empty revision to create a new file.","parameters":{"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"},"expected_revision":{"type":"string"}},"required":["path","text","expected_revision"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"run_command","description":"Run an approved command in the selected workspace with the user's account permissions. Windows uses PowerShell; SSH uses the host shell. Commands can access files outside the workspace and must be reviewed accordingly.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}}
    ]);
    schemas
        .as_array_mut()
        .expect("literal array")
        .extend(crate::app_tools::schemas());
    schemas
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("Missing tool argument: {key}"))
}
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn local_path(root: &Path, value: &str, create: bool) -> Result<PathBuf, String> {
    if value.contains('\0') {
        return Err("Invalid file path".into());
    }
    let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
    let input = PathBuf::from(value);
    let path = if input.is_absolute() {
        input
    } else {
        root.join(input)
    };
    let path = if create && !path.exists() {
        let parent = std::fs::canonicalize(path.parent().ok_or("No parent directory")?)
            .map_err(|e| e.to_string())?;
        parent.join(path.file_name().ok_or("No filename")?)
    } else {
        std::fs::canonicalize(path).map_err(|e| e.to_string())?
    };
    if !path.starts_with(&root) {
        return Err("The file is outside the selected workspace".into());
    }
    Ok(path)
}
fn operation(name: &str, path: String) -> RemoteFileRequest {
    RemoteFileRequest {
        operation: name.into(),
        path,
        local_path: None,
        destination: None,
        expected_revision: None,
        permissions: None,
        overwrite: false,
        recursive: false,
    }
}
fn remote_within(root: &str, path: &str) -> bool {
    root == "/" || path == root || path.strip_prefix(root).is_some_and(|s| s.starts_with('/'))
}
async fn remote_path(
    session: &SshSession,
    root: &str,
    path: &str,
    create: bool,
) -> Result<String, String> {
    if path.contains('\0') || path.is_empty() {
        return Err("Invalid remote path".into());
    }
    let root = session
        .file_operation(
            operation("stat", root.into()),
            Arc::new(FileProgress::default()),
        )
        .await
        .map_err(|e| e.to_string())?
        .metadata
        .ok_or("Workspace metadata missing")?
        .canonical_path;
    let candidate = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{path}", root.trim_end_matches('/'))
    };
    let canonical = if create {
        let (parent, name) = candidate.rsplit_once('/').ok_or("Invalid destination")?;
        if name.is_empty() || name == "." || name == ".." {
            return Err("Invalid destination name".into());
        }
        let parent = session
            .file_operation(
                operation(
                    "stat",
                    if parent.is_empty() {
                        "/".into()
                    } else {
                        parent.into()
                    },
                ),
                Arc::new(FileProgress::default()),
            )
            .await
            .map_err(|e| e.to_string())?
            .metadata
            .ok_or("Parent metadata missing")?;
        if parent.kind != "directory" {
            return Err("Parent is not a directory".into());
        }
        format!("{}/{name}", parent.canonical_path.trim_end_matches('/'))
    } else {
        session
            .file_operation(
                operation("stat", candidate),
                Arc::new(FileProgress::default()),
            )
            .await
            .map_err(|e| e.to_string())?
            .metadata
            .ok_or("File metadata missing")?
            .canonical_path
    };
    if !remote_within(&root, &canonical) {
        return Err("The file is outside the selected SSH workspace".into());
    }
    Ok(canonical)
}
pub async fn execute(workspace: Workspace, name: String, args: Value) -> Result<Value, String> {
    if name.starts_with("app_") {
        return crate::app_tools::execute(name, args).await;
    }
    match name.as_str() {
        "list_tools" => return crate::user_tools::list(),
        "install_tool" => {
            return crate::user_tools::install(
                args["manifest"].clone(),
                text(&args, "expected_revision")?,
            )
        }
        "run_tool" => {
            return Box::pin(crate::user_tools::run(
                text(&args, "id")?,
                text(&args, "expected_revision")?,
                serde_json::from_value(args["arguments"].clone())
                    .map_err(|_| "Tool arguments must be named strings")?,
                workspace,
            ))
            .await
        }
        _ => {}
    }
    if name == "run_command" {
        return execute_command(workspace, text(&args, "command")?).await;
    }
    let path = text(&args, "path")?;
    match workspace {
        Workspace::Local(root) => {
            let create = name == "write_file" && text(&args, "expected_revision")?.is_empty();
            let path = local_path(&root, path, create)?;
            match name.as_str() {
                "list_files" => {
                    let mut entries = tokio::fs::read_dir(path).await.map_err(|e| e.to_string())?;
                    let mut names = Vec::new();
                    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                        if names.len() == 2000 {
                            break;
                        }
                        names.push(entry.file_name().to_string_lossy().into_owned());
                    }
                    names.sort();
                    Ok(json!(names))
                }
                "read_file" => {
                    let metadata = tokio::fs::metadata(&path)
                        .await
                        .map_err(|e| e.to_string())?;
                    if metadata.len() > 256 * 1024 {
                        return Err("Read at most 256 KiB per file".into());
                    }
                    let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
                    Ok(
                        json!({"text":std::str::from_utf8(&bytes).map_err(|_|"This is not UTF-8 text")?,"revision":digest(&bytes)}),
                    )
                }
                "write_file" => {
                    let content = text(&args, "text")?.to_owned();
                    let revision = text(&args, "expected_revision")?.to_owned();
                    if content.len() > 256 * 1024 {
                        return Err("Write at most 256 KiB per file".into());
                    }
                    tokio::task::spawn_blocking(move || {
                        use std::io::Write;
                        let before = std::fs::read(&path).ok();
                        if before.as_ref().map(|v| digest(v)).unwrap_or_default() != revision {
                            return Err("Conflict: read the current file before saving".into());
                        }
                        let mut stage = tempfile::NamedTempFile::new_in(
                            path.parent().ok_or("No parent directory")?,
                        )
                        .map_err(|e| e.to_string())?;
                        if let Ok(metadata) = std::fs::metadata(&path) {
                            stage
                                .as_file()
                                .set_permissions(metadata.permissions())
                                .map_err(|e| e.to_string())?;
                        }
                        stage
                            .write_all(content.as_bytes())
                            .map_err(|e| e.to_string())?;
                        stage.as_file().sync_all().map_err(|e| e.to_string())?;
                        if create {
                            stage
                                .persist_noclobber(path)
                                .map_err(|e| e.error.to_string())?;
                        } else {
                            if std::fs::read(&path).map_err(|e| e.to_string())?
                                != before.ok_or("Original file missing")?
                            {
                                return Err("File changed during save".into());
                            }
                            stage.persist(path).map_err(|e| e.error.to_string())?;
                        }
                        Ok(json!({"saved":true,"revision":digest(content.as_bytes())}))
                    })
                    .await
                    .map_err(|e| e.to_string())?
                }
                _ => Err("Unknown tool".into()),
            }
        }
        Workspace::Ssh { session, root, .. } => {
            if session.is_closed() {
                return Err(
                    "The selected SSH connection is closed; no local fallback is allowed".into(),
                );
            }
            let create = name == "write_file" && text(&args, "expected_revision")?.is_empty();
            let path = remote_path(&session, &root, path, create).await?;
            let progress = Arc::new(FileProgress::default());
            if name == "list_files" {
                let result = session
                    .file_operation(operation("list", path), progress)
                    .await
                    .map_err(|e| e.to_string())?;
                return serde_json::to_value(result.entries).map_err(|e| e.to_string());
            }
            let stage = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
            let mut request = operation(
                if name == "read_file" {
                    "download"
                } else if create {
                    "createCopy"
                } else {
                    "save"
                },
                path.clone(),
            );
            request.local_path = Some(stage.path().to_string_lossy().into_owned());
            match name.as_str() {
                "read_file" => {
                    let metadata = session
                        .file_operation(operation("stat", path), Arc::new(FileProgress::default()))
                        .await
                        .map_err(|e| e.to_string())?
                        .metadata
                        .ok_or("File metadata missing")?;
                    if metadata.size.unwrap_or(u64::MAX) > 256 * 1024 {
                        return Err("Read at most 256 KiB per file".into());
                    }
                }
                "write_file" => {
                    let content = text(&args, "text")?;
                    if content.len() > 256 * 1024 {
                        return Err("Write at most 256 KiB per file".into());
                    }
                    request.expected_revision = Some(text(&args, "expected_revision")?.into());
                    tokio::fs::write(stage.path(), content)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                _ => return Err("Unknown tool".into()),
            }
            let result = session
                .file_operation(request, progress)
                .await
                .map_err(|e| e.to_string())?;
            if name == "read_file" {
                let bytes = tokio::fs::read(stage.path())
                    .await
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 256 * 1024 {
                    return Err("The file grew beyond the read limit".into());
                }
                Ok(
                    json!({"text":std::str::from_utf8(&bytes).map_err(|_|"This is not UTF-8 text")?,"revision":result.metadata.and_then(|v|v.revision)}),
                )
            } else {
                Ok(json!({"saved":true,"revision":result.metadata.and_then(|v|v.revision)}))
            }
        }
    }
}
async fn bounded_output(mut input: impl tokio::io::AsyncRead + Unpin) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = input.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > 128 * 1024 {
            return Err("Command output limit exceeded".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_labels_hide_verbatim_prefixes_without_changing_scope_identity() {
        for (raw, pretty) in [
            (r"\\?\C:\Users\rezae", r"C:\Users\rezae"),
            (r"\\?\UNC\server\share\folder", r"\\server\share\folder"),
            ("/home/AR/project", "/home/AR/project"),
        ] {
            let workspace = Workspace::Local(PathBuf::from(raw));
            assert_eq!(workspace.label(), pretty);
            assert_eq!(workspace.id(), format!("local:{raw}"));
        }
        let device = r"\\?\Volume{unchanged}\file";
        assert_eq!(display_path(Path::new(device)), device);
    }
    #[test]
    fn remote_scope_requires_a_directory_boundary() {
        assert!(remote_within("/home/a", "/home/a/file"));
        assert!(!remote_within("/home/a", "/home/ab/file"));
    }
    #[test]
    fn local_scope_refuses_parent_escape() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("inside");
        std::fs::create_dir(&inside).unwrap();
        std::fs::write(root.path().join("outside"), b"x").unwrap();
        assert!(local_path(&inside, "../outside", false).is_err());
    }
}

pub async fn execute_command(workspace: Workspace, command: &str) -> Result<Value, String> {
    if command.len() > 65536 || command.contains('\0') {
        return Err("Invalid or oversized command".into());
    }
    return match workspace {
        Workspace::Ssh { session, root, .. } => {
            if session.is_closed() {
                return Err("The chosen SSH connection is closed".into());
            }
            let root = remote_path(&session, &root, ".", false).await?;
            let quote = ad_desktop_model::files::quote_terminal_path;
            // This is reviewed shell source, not a filename: newlines are
            // valid here. Keep the script one literal argument to sh -lc.
            let script = format!("'{}'", command.replace('\'', "'\\''"));
            let command = format!("cd -- {} && exec sh -lc {}", quote(&root)?, script);
            let result = session
                .execute_scoped_command_output(&command, Duration::from_secs(60))
                .await
                .map_err(|e| e.to_string())?;
            Ok(
                json!({"stdout":String::from_utf8_lossy(&result.stdout[..result.stdout.len().min(128*1024)]),"stderr":String::from_utf8_lossy(&result.stderr[..result.stderr.len().min(128*1024)]),"exit_status":result.exit_status}),
            )
        }
        Workspace::Local(root) => {
            let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
            let system = std::env::var_os("SystemRoot").ok_or("Windows directory unavailable")?;
            let mut process = tokio::process::Command::new(
                PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            );
            let gated =
                format!("if ([Console]::In.ReadLine() -ne 'AETHER_RUN') {{ exit 125 }}\n{command}");
            process
                .args(["-NoProfile", "-NonInteractive", "-Command", &gated])
                .current_dir(root)
                .kill_on_drop(true)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(windows)]
            process.creation_flags(0x08000000);
            let mut child = process.spawn().map_err(|e| e.to_string())?;
            #[cfg(windows)]
            let _job = crate::process_job::ProcessJob::attach(&child)?;
            let mut gate = child.stdin.take().ok_or("Command input gate missing")?;
            gate.write_all(b"AETHER_RUN\n")
                .await
                .map_err(|e| e.to_string())?;
            drop(gate);
            let stdout = child.stdout.take().ok_or("Command output missing")?;
            let stderr = child.stderr.take().ok_or("Command errors missing")?;
            let result = tokio::time::timeout(Duration::from_secs(60), async {
                let (out, err) = tokio::try_join!(bounded_output(stdout), bounded_output(stderr))?;
                let status = child.wait().await.map_err(|e| e.to_string())?;
                Ok::<_, String>(json!({"stdout":out,"stderr":err,"exit_status":status.code()}))
            })
            .await
            .map_err(|_| "Command timed out".to_owned())?;
            if result.is_err() {
                let _ = child.kill().await;
            }
            result
        }
    };
}
