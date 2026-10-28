//! Runs from a temporary copy of the trusted, currently installed executable.
use crate::update_manifest::{self as metadata, Manifest};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub target: PathBuf,
    pub parent: u32,
    pub installer: bool,
    pub created: i64,
    /// Old plans always reopened the GUI. Exit updates deliberately stay closed.
    #[serde(default = "restart_by_default")]
    pub restart: bool,
}
fn restart_by_default() -> bool {
    true
}
fn error(_: impl std::fmt::Display) -> String {
    "Cannot prepare the application update. Check available disk space and folder permissions."
        .into()
}
pub fn no_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if let Ok(meta) = std::fs::symlink_metadata(ancestor) {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err("Update folders must not contain links or junctions".into());
                }
            }
            if meta.file_type().is_symlink() {
                return Err("Update folders must not contain links".into());
            }
        }
    }
    Ok(())
}
fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
fn powershell() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(
            std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?,
        )
        .join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
}
fn shell_path(path: &Path) -> PathBuf {
    // Rust canonical paths use the verbatim prefix on Windows. Legacy
    // PowerShell/Inno argument parsers expect ordinary drive/UNC syntax.
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}
#[cfg(windows)]
fn lock_path(target: &Path) -> Result<PathBuf, String> {
    let canonical = target.canonicalize().map_err(error)?;
    let value = canonical.to_string_lossy().to_lowercase();
    let hash: String = ring::digest::digest(&ring::digest::SHA256, value.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("Local application data unavailable")?,
    )
    .join("AetherDesk/update-locks");
    no_links(&root)?;
    std::fs::create_dir_all(&root).map_err(error)?;
    Ok(root.join(format!("{hash}.lock")))
}
/// Normal instances share a read handle. The updater needs an exclusive write
/// handle, so another running instance prevents replacement rather than being killed.
#[cfg(windows)]
pub fn running_guard() -> Result<Option<std::fs::File>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    let exe = std::env::current_exe().map_err(error)?;
    let path = lock_path(exe.parent().ok_or("Application folder unavailable")?)?;
    if !path.exists() {
        // A simultaneous first launch may create it first; then reopen for read.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
    }
    std::fs::OpenOptions::new().read(true).share_mode(1).open(path).map(Some)
        .map_err(|_| "An AetherDesk update is in progress, or its lock folder is unavailable. Please try again shortly.".into())
}
#[cfg(not(windows))]
pub fn running_guard() -> Result<Option<std::fs::File>, String> {
    Ok(None)
}
#[cfg(windows)]
fn installation_guard(target: &Path) -> Result<Option<std::fs::File>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(lock_path(target)?).map(Some)
        .map_err(|_| "Another AetherDesk window is open from this folder. Close it before installing the update.".into())
}
#[cfg(not(windows))]
fn installation_guard(_: &Path) -> Result<Option<std::fs::File>, String> {
    Err("Windows installation required".into())
}
#[cfg(windows)]
fn stage_guard(stage: &Path) -> Result<Option<std::fs::File>, String> {
    use std::os::windows::fs::OpenOptionsExt;
    no_links(stage)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(stage.join("apply.lock"))
        .map(Some)
        .map_err(|_| {
            "This downloaded update is already being prepared or installed. It remains queued."
                .into()
        })
}
#[cfg(not(windows))]
fn stage_guard(_: &Path) -> Result<Option<std::fs::File>, String> {
    Ok(None)
}

fn verify_payload(target: &Path, manifest: &Manifest) -> Result<(), String> {
    for file in &manifest.files {
        if !metadata::relative(&file.path) {
            return Err("Invalid update inventory path".into());
        }
        let path = target.join(&file.path);
        no_links(&path)?;
        if std::fs::metadata(&path).map_err(error)?.len() != file.bytes
            || metadata::file_hash(&path)? != file.sha256.to_ascii_lowercase()
        {
            return Err(format!(
                "Installed update verification failed: {}",
                file.path
            ));
        }
    }
    Ok(())
}
pub fn stage_apply(stage: &Path, installer: bool) -> Result<(), String> {
    stage_apply_mode(stage, installer, true)
}
pub fn stage_apply_on_exit(stage: &Path, installer: bool) -> Result<(), String> {
    if crate::update_exit::system_ending() {
        return Err("Update deferred while Windows is shutting down".into());
    }
    stage_apply_mode(stage, installer, false)
}
fn stage_apply_mode(stage: &Path, installer: bool, restart: bool) -> Result<(), String> {
    let _stage_guard = stage_guard(stage)?;
    let exe = std::env::current_exe().map_err(error)?;
    let target = exe
        .parent()
        .ok_or("Application folder unavailable")?
        .canonicalize()
        .map_err(error)?;
    no_links(&target)?;
    no_links(stage)?;
    let helper = stage.join("AetherDesk-updater.exe");
    std::fs::copy(&exe, &helper).map_err(error)?;
    // This executable links the bundled media libraries even in helper mode.
    // Load isolated copies so the helper does not lock the DLLs being replaced.
    for entry in std::fs::read_dir(&target).map_err(error)? {
        let entry = entry.map_err(error)?;
        if entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
        {
            no_links(&entry.path())?;
            if entry.file_type().map_err(error)?.is_file() {
                std::fs::copy(entry.path(), stage.join(entry.file_name())).map_err(error)?;
            }
        }
    }
    let plan = Plan {
        target,
        parent: std::process::id(),
        installer,
        created: chrono::Utc::now().timestamp(),
        restart,
    };
    let plan_bytes = serde_json::to_vec(&plan).map_err(error)?;
    publish_file(&stage.join("plan.json"), |output| {
        std::io::Write::write_all(output, &plan_bytes)
    })?;
    let _ = std::fs::remove_file(stage.join("deferred.txt"));
    hidden(
        Command::new(&helper)
            .arg("--apply-update")
            .arg(stage.join("plan.json"))
            .current_dir(stage),
    )
    .spawn()
    .map_err(error)?;
    Ok(())
}
#[cfg(windows)]
fn wait_parent(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject},
    };
    unsafe {
        let handle = OpenProcess(0x00100000, 0, pid);
        if handle.is_null() {
            // The application may already have exited before this helper starts.
            if std::io::Error::last_os_error().raw_os_error() == Some(87) {
                return Ok(());
            }
            return Err("Cannot identify the application being updated".into());
        }
        let result = WaitForSingleObject(handle, 120_000);
        CloseHandle(handle);
        if result != 0 {
            return Err("AetherDesk did not close. The update was cancelled.".into());
        }
    }
    Ok(())
}
#[cfg(not(windows))]
fn wait_parent(_: u32) -> Result<(), String> {
    Err("Automatic installation is supported on Windows only".into())
}

const EXTRACT: &str = r#"param([string]$Archive,[string]$Destination,[string]$Inventory)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$expected=@{}
foreach($file in (Get-Content -LiteralPath $Inventory -Raw | ConvertFrom-Json)) { $expected[$file.path]=$file }
$archiveFile=[IO.Compression.ZipFile]::OpenRead($Archive)
try {
  $seen=@{}
  foreach($entry in $archiveFile.Entries) {
    $name=$entry.FullName
    if(!$expected.ContainsKey($name) -or $seen.ContainsKey($name) -or $entry.Length -ne $expected[$name].bytes -or
       (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) { throw 'Unexpected archive entry' }
    $seen[$name]=$true
  }
  if($seen.Count -ne $expected.Count) { throw 'Incomplete archive' }
  foreach($entry in $archiveFile.Entries) {
    $path=Join-Path $Destination $entry.FullName
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path)) | Out-Null
    [IO.Compression.ZipFileExtensions]::ExtractToFile($entry,$path,$false)
  }
} finally { $archiveFile.Dispose() }
"#;
fn extract(stage: &Path, manifest: &Manifest) -> Result<PathBuf, String> {
    let destination = stage.join("unpacked");
    // A fresh stage is used for every download; never reuse partial extraction.
    std::fs::create_dir(&destination).map_err(error)?;
    let script = stage.join("extract.ps1");
    std::fs::write(&script, EXTRACT).map_err(error)?;
    let inventory = stage.join("files.json");
    std::fs::write(
        &inventory,
        serde_json::to_vec(&manifest.files).map_err(error)?,
    )
    .map_err(error)?;
    let status = hidden(
        Command::new(powershell()?)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(shell_path(&script))
            .arg(shell_path(&stage.join(&manifest.portable.name)))
            .arg(shell_path(&destination))
            .arg(shell_path(&inventory)),
    )
    .status()
    .map_err(error)?;
    if !status.success() {
        return Err("The update archive is damaged or unsafe".into());
    }
    for file in &manifest.files {
        let path = destination.join(&file.path);
        no_links(&path)?;
        if std::fs::metadata(&path).map_err(error)?.len() != file.bytes
            || metadata::file_hash(&path)? != file.sha256.to_ascii_lowercase()
        {
            return Err("An extracted update file failed verification".into());
        }
    }
    Ok(destination)
}

fn file_error(action: &str, path: &Path, cause: impl std::fmt::Display) -> String {
    format!("Cannot {action} '{}': {cause}", path.display())
}

// Windows CopyFile refuses to overwrite a read-only destination, including the
// Linux runtime files shipped in portable packages. Copying their permissions to
// a backup also made the old recovery path fail in exactly the same way.
fn writable_owned_file(path: &Path) -> Result<Option<std::fs::Permissions>, String> {
    #[cfg(windows)]
    match std::fs::metadata(path) {
        Ok(meta) if meta.permissions().readonly() => {
            let original = meta.permissions();
            let mut writable = original.clone();
            writable.set_readonly(false);
            std::fs::set_permissions(path, writable)
                .map_err(|e| file_error("make the package file writable", path, e))?;
            return Ok(Some(original));
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(file_error("inspect the package file", path, e));
        }
        _ => {}
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(None)
}

fn restore_permissions(
    path: &Path,
    permissions: Option<std::fs::Permissions>,
) -> Result<(), String> {
    if let Some(permissions) = permissions {
        std::fs::set_permissions(path, permissions)
            .map_err(|e| file_error("restore file permissions", path, e))?;
    }
    Ok(())
}

// Complete and flush a sibling file before publishing it. A copy/space failure
// leaves the old destination intact; the temporary file is removed on failure.
// Byte copies intentionally do not propagate Windows read-only attributes.
fn publish_file(
    destination: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(), String> {
    no_links(destination)?;
    let parent = destination.parent().ok_or("Invalid update destination")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".aetherdesk-update-")
        .tempfile_in(parent)
        .map_err(|e| file_error("prepare a replacement for", destination, e))?;
    write(temporary.as_file_mut())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| file_error("write a replacement for", destination, e))?;
    let permissions = writable_owned_file(destination)?;
    let mut attempts = 0;
    loop {
        match temporary.persist(destination) {
            Ok(_) => return Ok(()),
            Err(failure) => {
                let retry = cfg!(windows)
                    && matches!(failure.error.raw_os_error(), Some(5 | 32 | 33))
                    && attempts < 10;
                if !retry {
                    restore_permissions(destination, permissions)?;
                    return Err(file_error(
                        "replace the package file",
                        destination,
                        failure.error,
                    ));
                }
                temporary = failure.file;
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

fn copy_owned_file(source: &Path, destination: &Path) -> Result<(), String> {
    no_links(source)?;
    publish_file(destination, |output| {
        std::io::copy(&mut std::fs::File::open(source)?, output).map(|_| ())
    })
}

fn remove_owned_file(path: &Path) -> Result<(), String> {
    no_links(path)?;
    let permissions = writable_owned_file(path)?;
    if let Err(cause) = std::fs::remove_file(path) {
        restore_permissions(path, permissions)?;
        return Err(file_error("remove the obsolete package file", path, cause));
    }
    Ok(())
}

fn backup_package(
    target: &Path,
    stage: &Path,
    manifest: &Manifest,
    installer: bool,
) -> Result<Vec<(String, bool)>, String> {
    let backup = stage.join("previous");
    std::fs::create_dir(&backup).map_err(error)?;
    let mut paths: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();
    // Remove obsolete files only when a previous signed manifest proves ownership.
    if let Ok(previous) = std::fs::read(target.join("installed-update.json")) {
        if let Ok(previous) = metadata::verify(
            &previous,
            &metadata::config()?.public_key,
            chrono::Utc::now().timestamp(),
        ) {
            for file in previous.files {
                if !paths
                    .iter()
                    .any(|path| path.eq_ignore_ascii_case(&file.path))
                {
                    paths.push(file.path);
                }
            }
        }
    }
    if installer {
        for name in ["unins000.exe", "unins000.dat"] {
            if target.join(name).is_file() {
                paths.push(name.into());
            }
        }
    }
    paths.push("installed-update.json".into());
    let mut journal = Vec::new();
    for path in &paths {
        let destination = target.join(path);
        no_links(&destination)?;
        let existed = destination.exists();
        if existed {
            if !destination.is_file() {
                return Err("An update file conflicts with an existing folder".into());
            }
            let saved = backup.join(path);
            std::fs::create_dir_all(saved.parent().ok_or("Invalid backup path")?).map_err(error)?;
            copy_owned_file(&destination, &saved)?;
        }
        journal.push((path.clone(), existed));
    }
    let journal_bytes = serde_json::to_vec(&journal).map_err(error)?;
    publish_file(&stage.join("rollback.json"), |output| {
        std::io::Write::write_all(output, &journal_bytes)
    })?;
    Ok(journal)
}

/// A durable journal is retained beside the update until installation succeeds.
/// Only signed package paths are touched; unknown files and user data are kept.
fn replace(
    target: &Path,
    source: &Path,
    stage: &Path,
    manifest: &Manifest,
    envelope: &[u8],
) -> Result<(), String> {
    let journal = backup_package(target, stage, manifest, false)?;
    let outcome = (|| {
        for file in &manifest.files {
            let destination = target.join(&file.path);
            std::fs::create_dir_all(destination.parent().ok_or("Invalid update path")?)
                .map_err(error)?;
            copy_owned_file(&source.join(&file.path), &destination)?;
        }
        for (path, existed) in &journal {
            if *existed
                && path != "installed-update.json"
                && !manifest
                    .files
                    .iter()
                    .any(|f| f.path.eq_ignore_ascii_case(path))
            {
                remove_owned_file(&target.join(path))?;
            }
        }
        publish_file(&target.join("installed-update.json"), |output| {
            std::io::Write::write_all(output, envelope)
        })?;
        Ok::<_, String>(())
    })();
    if let Err(cause) = outcome {
        let _ = std::fs::write(stage.join("replacement-error.txt"), &cause);
        rollback(target, stage, &journal)?;
        return Err(format!(
            "The update failed. The previous files were restored.\n{cause}"
        ));
    }
    Ok(())
}
fn rollback(target: &Path, stage: &Path, journal: &[(String, bool)]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    if journal
        .iter()
        .any(|(path, _)| !metadata::relative(path) || !seen.insert(path.to_ascii_lowercase()))
    {
        return Err("Unsafe or duplicate recovery journal entry".into());
    }
    // Validate the complete journal before touching anything. Then attempt every
    // restore, so one locked file cannot leave all earlier files unrestored.
    if journal.iter().any(|(path, _)| !metadata::relative(path)) {
        return Err("Invalid rollback path".into());
    }
    let completed = stage.join("rollback-complete");
    if completed.exists() {
        std::fs::remove_file(&completed).map_err(error)?;
    }
    let mut failures = Vec::new();
    for (path, existed) in journal.iter().rev() {
        let destination = target.join(path);
        let outcome = (|| {
            no_links(&destination)?;
            if *existed {
                copy_owned_file(&stage.join("previous").join(path), &destination)?;
            } else if destination.is_file() {
                remove_owned_file(&destination)?;
            }
            Ok::<_, String>(())
        })();
        if let Err(cause) = outcome {
            failures.push(cause);
        }
    }
    if !failures.is_empty() {
        let _ = std::fs::write(stage.join("recovery-errors.txt"), failures.join("\n"));
        return Err(format!(
            "Automatic recovery could not finish ({} files). Keep the update recovery folder: {}\n{}",
            failures.len(), stage.display(), failures[0]
        ));
    }
    std::fs::write(completed, b"restored").map_err(error)?;
    Ok(())
}
fn apply(plan_path: &Path) -> Result<(), String> {
    let stage = plan_path
        .parent()
        .ok_or("Invalid update plan")?
        .canonicalize()
        .map_err(error)?;
    no_links(&stage)?;
    let stage_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let _stage_guard = loop {
        match stage_guard(&stage) {
            Ok(guard) => break guard,
            Err(_) if std::time::Instant::now() < stage_deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Err(error) => return Err(error),
        }
    };
    let plan: Plan =
        serde_json::from_slice(&std::fs::read(plan_path).map_err(error)?).map_err(error)?;
    if plan.parent == 0 || chrono::Utc::now().timestamp().abs_diff(plan.created) > 180 {
        return Err("The update installation request has expired".into());
    }
    no_links(&plan.target)?;
    if !plan.target.is_absolute()
        || !plan.target.join("AetherDesk.exe").is_file()
        || plan.target.starts_with(&stage)
        || stage.starts_with(&plan.target)
    {
        return Err("Invalid application update destination".into());
    }
    let config = metadata::config()?;
    let envelope = std::fs::read(stage.join("update.json")).map_err(error)?;
    let manifest = metadata::verify(
        &envelope,
        &config.public_key,
        chrono::Utc::now().timestamp(),
    )?;
    if metadata::version(&manifest.version, manifest.build)? <= metadata::current()? {
        return Err("An older update cannot be installed".into());
    }
    let existing: serde_json::Value =
        serde_json::from_slice(&std::fs::read(plan.target.join("release.json")).map_err(error)?)
            .map_err(error)?;
    let version = existing["version"]
        .as_str()
        .ok_or("Existing version unavailable")?;
    let mut parts: Vec<_> = version.split('.').collect();
    while parts.len() < 3 {
        parts.push("0");
    }
    if metadata::version(&manifest.version, manifest.build)?
        <= metadata::version(
            &parts.join("."),
            existing["build"]
                .as_u64()
                .ok_or("Existing build unavailable")?,
        )?
    {
        return Err("The application already has this update or a newer version".into());
    }
    let asset = if plan.installer {
        manifest
            .installer
            .as_ref()
            .ok_or("Installer update unavailable")?
    } else {
        &manifest.portable
    };
    let package = stage.join(&asset.name);
    if std::fs::metadata(&package).map_err(error)?.len() != asset.bytes
        || metadata::file_hash(&package)? != asset.sha256.to_ascii_lowercase()
    {
        return Err("The downloaded update failed verification".into());
    }
    // Keep a real process handle while waiting so a recycled PID cannot match.
    wait_parent(plan.parent)?;
    let wait_deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(if plan.restart { 0 } else { 30 });
    let guard = loop {
        match installation_guard(&plan.target) {
            Ok(guard) => break guard,
            Err(_) if !plan.restart && std::time::Instant::now() < wait_deadline => {
                std::thread::sleep(std::time::Duration::from_millis(200))
            }
            Err(message) if !plan.restart => {
                std::fs::write(stage.join("deferred.txt"), message).map_err(error)?;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    };
    if plan.installer {
        let journal = backup_package(&plan.target, &stage, &manifest, true)?;
        let status = hidden(
            Command::new(package)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
                .arg(format!("/DIR={}", shell_path(&plan.target).display()))
                .arg(format!(
                    "/LOG={}",
                    shell_path(&stage.join("installer.log")).display()
                )),
        )
        .status();
        if !status.is_ok_and(|status| status.success()) {
            rollback(&plan.target, &stage, &journal)?;
            return Err(
                "The installer could not finish. Previous application files were restored; keep its log and recovery folder.".into(),
            );
        }
        publish_file(&plan.target.join("installed-update.json"), |output| {
            std::io::Write::write_all(output, &envelope)
        })?;
    } else {
        let unpacked = match extract(&stage, &manifest) {
            Ok(unpacked) => unpacked,
            Err(cause) => {
                drop(guard);
                if plan.restart {
                    restart(&plan.target)?;
                }
                return Err(cause);
            }
        };
        if let Err(cause) = replace(&plan.target, &unpacked, &stage, &manifest, &envelope) {
            drop(guard);
            if !stage.join("rollback.json").exists() || stage.join("rollback-complete").is_file() {
                if plan.restart {
                    restart(&plan.target)?;
                }
            }
            return Err(cause);
        }
    }
    if let Err(cause) = verify_payload(&plan.target, &manifest) {
        let journal: Vec<(String, bool)> =
            serde_json::from_slice(&std::fs::read(stage.join("rollback.json")).map_err(error)?)
                .map_err(error)?;
        rollback(&plan.target, &stage, &journal)?;
        return Err(cause);
    }
    if !plan.restart {
        // Retain the plan/journal/backup until the user's next rendered launch.
        // No GUI is opened just to acknowledge a background installation.
        defer_health(&stage)?;
        drop(guard);
        return Ok(());
    }
    drop(guard);
    let launched = hidden(
        Command::new(plan.target.join("AetherDesk.exe"))
            .arg("--update-health")
            .arg(&stage)
            .current_dir(&plan.target),
    )
    .spawn();
    let mut child =
        match launched {
            Ok(child) => child,
            Err(_) if !plan.installer => {
                restore_previous(&plan, &stage)?;
                return Err(
                "The new application could not start. The previous portable version was restored."
                    .into(),
            );
            }
            Err(_) => return Err(
                "The installed application could not start. Keep the installer log for recovery."
                    .into(),
            ),
        };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while !stage.join("healthy").is_file() {
        if child.try_wait().map_err(error)?.is_some() || std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            if !plan.installer {
                restore_previous(&plan, &stage)?;
                return Err("The updated application did not open successfully. The previous portable version was restored.".into());
            }
            return Err("The installer finished, but AetherDesk did not confirm a successful start. Keep the update folder and installer log for recovery.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    std::fs::write(stage.join("complete"), b"ok").map_err(error)?;
    Ok(())
}
fn defer_health(stage: &Path) -> Result<(), String> {
    publish_file(&stage.join("awaiting-health"), |output| {
        std::io::Write::write_all(output, b"next launch")
    })?;
    let instructions = format!("AetherDesk was updated while closed. Backups remain here until its next successful launch.\nIf the application cannot start, close all AetherDesk windows and run:\n\"{}\" --recover-update \"{}\"\nKeep installer.log if an installed copy requires installer repair.\n", stage.join("AetherDesk-updater.exe").display(), stage.join("plan.json").display());
    publish_file(&stage.join("RECOVERY.txt"), |output| {
        std::io::Write::write_all(output, instructions.as_bytes())
    })?;
    Ok(())
}

fn restore_previous(plan: &Plan, stage: &Path) -> Result<(), String> {
    let guard = installation_guard(&plan.target)?;
    let journal: Vec<(String, bool)> =
        serde_json::from_slice(&std::fs::read(stage.join("rollback.json")).map_err(error)?)
            .map_err(error)?;
    rollback(&plan.target, stage, &journal)?;
    drop(guard);
    if plan.restart {
        restart(&plan.target)
    } else {
        Ok(())
    }
}
fn restart(target: &Path) -> Result<(), String> {
    hidden(Command::new(target.join("AetherDesk.exe")).current_dir(target))
        .spawn()
        .map_err(|e| file_error("restart the restored application", target, e))?;
    Ok(())
}
pub fn mark_healthy(stage: &Path) -> Result<(), String> {
    validate_health_target(stage)?;
    let plan: Plan =
        serde_json::from_slice(&std::fs::read(stage.join("plan.json")).map_err(error)?)
            .map_err(error)?;
    record_health(stage, plan.restart)
}
fn record_health(stage: &Path, restart: bool) -> Result<(), String> {
    std::fs::write(stage.join("healthy"), b"rendered").map_err(error)?;
    if !restart && stage.join("awaiting-health").is_file() {
        publish_file(&stage.join("complete"), |output| {
            std::io::Write::write_all(output, b"rendered")
        })?;
    }
    Ok(())
}

pub fn pending_health() -> Result<Option<PathBuf>, String> {
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Missing update cache")?)
        .join("AetherDesk/updates");
    if !root.exists() {
        return Ok(None);
    }
    no_links(&root)?;
    let target = std::env::current_exe()
        .map_err(error)?
        .parent()
        .ok_or("Missing application directory")?
        .canonicalize()
        .map_err(error)?;
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(error)?.flatten() {
        if !entry.file_type().map_err(error)?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("update-")
        {
            continue;
        }
        let stage = entry.path();
        if stage.join("complete").exists()
            || stage.join("error.txt").exists()
            || !stage.join("awaiting-health").is_file()
        {
            continue;
        }
        no_links(&stage)?;
        let Ok(bytes) = std::fs::read(stage.join("plan.json")) else {
            continue;
        };
        if let Ok(plan) = serde_json::from_slice::<Plan>(&bytes) {
            if !plan.restart && plan.target == target {
                candidates.push((plan.created, stage));
            }
        }
    }
    candidates.sort_by_key(|(created, _)| *created);
    Ok(candidates.pop().map(|(_, path)| path))
}
pub fn validate_health_target(stage: &Path) -> Result<(), String> {
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Missing cache directory")?)
        .join("AetherDesk/updates");
    no_links(stage)?;
    let stage = stage.canonicalize().map_err(error)?;
    if stage.parent() != Some(root.canonicalize().map_err(error)?.as_path()) {
        return Err("Invalid update health directory".into());
    }
    let plan: Plan =
        serde_json::from_slice(&std::fs::read(stage.join("plan.json")).map_err(error)?)
            .map_err(error)?;
    let exe = std::env::current_exe().map_err(error)?;
    if exe
        .parent()
        .ok_or("Application directory unavailable")?
        .canonicalize()
        .map_err(error)?
        != plan.target
    {
        return Err("Update health target does not match this application".into());
    }
    let manifest = metadata::verify(
        &std::fs::read(stage.join("update.json")).map_err(error)?,
        &metadata::config()?.public_key,
        chrono::Utc::now().timestamp(),
    )?;
    if metadata::version(&manifest.version, manifest.build)? != metadata::current()? {
        return Err("Update health version does not match this application".into());
    }
    Ok(())
}
pub fn entry(path: &Path) {
    let background = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Plan>(&bytes).ok())
        .is_some_and(|plan| !plan.restart);
    if let Err(message) = apply(path) {
        if let Some(stage) = path.parent() {
            let _ = std::fs::write(stage.join("error.txt"), &message);
        }
        #[cfg(windows)]
        if !background {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW;
                let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
                MessageBoxW(
                    std::ptr::null_mut(),
                    wide(&message).as_ptr(),
                    wide("AetherDesk update").as_ptr(),
                    0x10,
                );
            }
        }
    }
}

/// Explicit user-invoked recovery. Never opens the GUI or installs a package.
pub fn recover_entry(path: &Path) {
    let result = (|| -> Result<(), String> {
        let stage = path
            .parent()
            .ok_or("Missing recovery directory")?
            .canonicalize()
            .map_err(error)?;
        no_links(&stage)?;
        let own = std::env::current_exe()
            .map_err(error)?
            .parent()
            .ok_or("Missing updater directory")?
            .canonicalize()
            .map_err(error)?;
        if stage != own {
            return Err("Recovery must run from its original updater directory".into());
        }
        let mut plan: Plan =
            serde_json::from_slice(&std::fs::read(path).map_err(error)?).map_err(error)?;
        no_links(&plan.target)?;
        if !plan.target.is_absolute()
            || plan.target.starts_with(&stage)
            || stage.starts_with(&plan.target)
        {
            return Err("Invalid recovery target".into());
        }
        let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Missing update cache")?)
            .join("AetherDesk/updates");
        if stage.parent() != Some(root.canonicalize().map_err(error)?.as_path()) {
            return Err("Invalid recovery stage".into());
        }
        let _guard = stage_guard(&stage)?;
        let envelope = std::fs::read(stage.join("update.json")).map_err(error)?;
        metadata::verify(&envelope, &metadata::config()?.public_key, plan.created)?;
        plan.restart = false;
        restore_previous(&plan, &stage)?;
        std::fs::write(stage.join("error.txt"), "Previous application files were restored by explicit recovery. Installed copies may require installer repair.").map_err(error)?;
        Ok(())
    })();
    let message = match result {
        Ok(()) => "Previous application files restored. You can open AetherDesk again.".into(),
        Err(error) => error,
    };
    #[cfg(windows)]
    unsafe {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            wide(&message).as_ptr(),
            wide("AetherDesk recovery").as_ptr(),
            0x40,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_update_plans_stay_closed_and_keep_backups_until_next_rendered_launch() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("previous")).unwrap();
        std::fs::write(
            root.path().join("previous/AetherDesk.exe"),
            b"old executable",
        )
        .unwrap();
        let legacy:Plan=serde_json::from_value(serde_json::json!({"target":"C:/Apps/AetherDesk","parent":42,"installer":false,"created":1})).unwrap();
        assert!(legacy.restart, "old restart-now plans changed behavior");
        let background:Plan=serde_json::from_value(serde_json::json!({"target":"C:/Apps/AetherDesk","parent":42,"installer":false,"created":1,"restart":false})).unwrap();
        assert!(!background.restart);
        defer_health(root.path()).unwrap();
        assert!(!root.path().join("complete").exists());
        assert!(!root.path().join("healthy").exists());
        assert!(root.path().join("RECOVERY.txt").is_file());
        record_health(root.path(), background.restart).unwrap();
        assert!(root.path().join("complete").is_file());
        assert_eq!(
            std::fs::read(root.path().join("previous/AetherDesk.exe")).unwrap(),
            b"old executable"
        );
        let restart = tempfile::tempdir().unwrap();
        record_health(restart.path(), true).unwrap();
        assert!(restart.path().join("healthy").exists());
        assert!(
            !restart.path().join("complete").exists(),
            "restart helper must retain ownership of completion"
        );
    }
    #[test]
    fn installer_recovery_preserves_application_and_uninstaller_files_and_rejects_bad_inventory() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("application");
        let stage = root.path().join("stage");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(&stage).unwrap();
        for name in ["AetherDesk.exe", "unins000.exe", "unins000.dat"] {
            std::fs::write(target.join(name), b"previous").unwrap();
        }
        std::fs::write(target.join("user.txt"), b"keep").unwrap();
        let manifest = Manifest {
            schema: 1,
            product: "AetherDesk".into(),
            target: "windows-x64".into(),
            version: "0.1.3".into(),
            build: 99,
            published: 0,
            expires: i64::MAX,
            notes: String::new(),
            portable: metadata::Asset {
                name: "update.zip".into(),
                bytes: 1,
                sha256: "0".repeat(64),
            },
            installer: None,
            files: vec![metadata::File {
                path: "AetherDesk.exe".into(),
                bytes: 8,
                sha256: metadata::file_hash(&target.join("AetherDesk.exe")).unwrap(),
            }],
        };
        verify_payload(&target, &manifest).unwrap();
        let journal = backup_package(&target, &stage, &manifest, true).unwrap();
        for name in ["AetherDesk.exe", "unins000.exe", "unins000.dat"] {
            std::fs::write(target.join(name), b"new").unwrap();
        }
        assert!(verify_payload(&target, &manifest).is_err());
        rollback(&target, &stage, &journal).unwrap();
        for name in ["AetherDesk.exe", "unins000.exe", "unins000.dat"] {
            assert_eq!(std::fs::read(target.join(name)).unwrap(), b"previous");
        }
        assert_eq!(std::fs::read(target.join("user.txt")).unwrap(), b"keep");
        assert!(rollback(&target, &stage, &[("../outside".into(), true)]).is_err());
    }
    fn read_only(path: &Path, value: bool) {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(value);
        std::fs::set_permissions(path, permissions).unwrap();
    }
    #[test]
    #[cfg(windows)]
    fn portable_replacement_and_recovery_handle_readonly_runtime_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("OneDrive - Test/Aether");
        let source = dir.path().join("unpacked");
        let stage = dir.path().join("update");
        for root in [&target, &source, &stage] {
            std::fs::create_dir_all(root.join("agent/libs")).unwrap();
        }
        let paths = ["AetherDesk.exe", "agent/libs/libglib.so"];
        for path in paths {
            std::fs::write(target.join(path), b"old package").unwrap();
            std::fs::write(source.join(path), b"new package").unwrap();
            read_only(&target.join(path), true);
            read_only(&source.join(path), true);
        }
        std::fs::write(target.join("user.txt"), b"preserve user data").unwrap();
        read_only(&target.join("user.txt"), true);
        let manifest = Manifest {
            schema: 1,
            product: "AetherDesk".into(),
            target: "windows-x64".into(),
            version: "0.1.2".into(),
            build: 5,
            published: 0,
            expires: i64::MAX,
            notes: String::new(),
            installer: None,
            portable: metadata::Asset {
                name: "fixture.zip".into(),
                bytes: 0,
                sha256: String::new(),
            },
            files: paths
                .iter()
                .map(|path| metadata::File {
                    path: (*path).into(),
                    bytes: 11,
                    sha256: String::new(),
                })
                .collect(),
        };
        replace(&target, &source, &stage, &manifest, b"fixture manifest").unwrap();
        assert_eq!(
            std::fs::read(target.join("installed-update.json")).unwrap(),
            b"fixture manifest"
        );
        for path in paths {
            assert_eq!(std::fs::read(target.join(path)).unwrap(), b"new package");
            assert!(!std::fs::metadata(target.join(path))
                .unwrap()
                .permissions()
                .readonly());
            assert_eq!(
                std::fs::read(stage.join("previous").join(path)).unwrap(),
                b"old package"
            );
            read_only(&target.join(path), true);
            read_only(&stage.join("previous").join(path), true);
        }
        let journal = serde_json::from_slice::<Vec<(String, bool)>>(
            &std::fs::read(stage.join("rollback.json")).unwrap(),
        )
        .unwrap();
        rollback(&target, &stage, &journal).unwrap();
        for path in paths {
            assert_eq!(std::fs::read(target.join(path)).unwrap(), b"old package");
            read_only(&source.join(path), false);
            read_only(&stage.join("previous").join(path), false);
        }
        assert!(!target.join("installed-update.json").exists());
        assert_eq!(
            std::fs::read(target.join("user.txt")).unwrap(),
            b"preserve user data"
        );
        assert!(std::fs::metadata(target.join("user.txt"))
            .unwrap()
            .permissions()
            .readonly());
        read_only(&target.join("user.txt"), false);
    }
    #[test]
    fn incomplete_replacement_write_keeps_original_bytes_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("runtime.dll");
        std::fs::write(&destination, b"complete original").unwrap();
        read_only(&destination, true);
        let result = publish_file(&destination, |file| {
            std::io::Write::write_all(file, b"incomplete")?;
            Err(std::io::Error::other("simulated full disk"))
        });
        assert!(result.unwrap_err().contains("simulated full disk"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"complete original");
        assert!(std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .readonly());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        read_only(&destination, false);
    }
    #[test]
    #[cfg(windows)]
    fn recovery_attempts_other_files_when_one_is_locked_and_records_the_path() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("application");
        let stage = dir.path().join("update");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir_all(stage.join("previous")).unwrap();
        for name in ["AetherDesk.exe", "locked.dll"] {
            std::fs::write(target.join(name), b"new").unwrap();
            std::fs::write(stage.join("previous").join(name), b"old").unwrap();
        }
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(target.join("locked.dll"))
            .unwrap();
        let journal = [("AetherDesk.exe".into(), true), ("locked.dll".into(), true)];
        assert!(rollback(&target, &stage, &journal)
            .unwrap_err()
            .contains("locked.dll"));
        assert_eq!(
            std::fs::read(target.join("AetherDesk.exe")).unwrap(),
            b"old"
        );
        assert!(!stage.join("rollback-complete").exists());
        assert!(std::fs::read_to_string(stage.join("recovery-errors.txt"))
            .unwrap()
            .contains("locked.dll"));
        drop(locked);
        rollback(&target, &stage, &journal).unwrap();
        assert_eq!(std::fs::read(target.join("locked.dll")).unwrap(), b"old");
        assert!(stage.join("rollback-complete").is_file());
    }
    #[test]
    fn interrupted_portable_replacement_can_restore_previous_files_without_touching_user_data() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("application");
        let stage = dir.path().join("stage");
        std::fs::create_dir_all(stage.join("previous")).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("AetherDesk.exe"), b"new").unwrap();
        std::fs::write(target.join("user.txt"), b"keep").unwrap();
        std::fs::write(stage.join("previous/AetherDesk.exe"), b"old").unwrap();
        std::fs::write(target.join("new.dll"), b"new").unwrap();
        rollback(
            &target,
            &stage,
            &[("AetherDesk.exe".into(), true), ("new.dll".into(), false)],
        )
        .unwrap();
        assert_eq!(
            std::fs::read(target.join("AetherDesk.exe")).unwrap(),
            b"old"
        );
        assert_eq!(std::fs::read(target.join("user.txt")).unwrap(), b"keep");
        assert!(!target.join("new.dll").exists());
        assert!(rollback(&target, &stage, &[("../user.txt".into(), false)]).is_err());
    }
}
