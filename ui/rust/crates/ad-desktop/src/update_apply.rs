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
pub fn stage_apply(stage: &Path, installer: bool) -> Result<(), String> {
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
    };
    std::fs::write(
        stage.join("plan.json"),
        serde_json::to_vec(&plan).map_err(error)?,
    )
    .map_err(error)?;
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

/// A durable journal is retained beside the update until installation succeeds.
/// Only signed package paths are touched; unknown files and user data are kept.
fn replace(
    target: &Path,
    source: &Path,
    stage: &Path,
    manifest: &Manifest,
    envelope: &[u8],
) -> Result<(), String> {
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
            std::fs::copy(&destination, saved).map_err(error)?;
        }
        journal.push((path.clone(), existed));
    }
    std::fs::write(
        stage.join("rollback.json"),
        serde_json::to_vec(&journal).map_err(error)?,
    )
    .map_err(error)?;
    let outcome = (|| {
        for file in &manifest.files {
            let destination = target.join(&file.path);
            std::fs::create_dir_all(destination.parent().ok_or("Invalid update path")?)
                .map_err(error)?;
            std::fs::copy(source.join(&file.path), &destination).map_err(error)?;
        }
        for (path, existed) in &journal {
            if *existed
                && path != "installed-update.json"
                && !manifest
                    .files
                    .iter()
                    .any(|f| f.path.eq_ignore_ascii_case(path))
            {
                std::fs::remove_file(target.join(path)).map_err(error)?;
            }
        }
        std::fs::write(target.join("installed-update.json"), envelope).map_err(error)?;
        Ok::<_, String>(())
    })();
    if outcome.is_err() {
        rollback(target, stage, &journal)?;
        return Err("The update failed. The previous files were restored.".into());
    }
    Ok(())
}
fn rollback(target: &Path, stage: &Path, journal: &[(String, bool)]) -> Result<(), String> {
    for (path, existed) in journal.iter().rev() {
        if !metadata::relative(path) {
            return Err("Invalid rollback path".into());
        }
        let destination = target.join(path);
        no_links(&destination)?;
        if *existed {
            std::fs::copy(stage.join("previous").join(path), destination).map_err(|_| {
                "Automatic recovery could not finish. Keep the update recovery folder.".to_string()
            })?;
        } else if destination.is_file() {
            std::fs::remove_file(destination).map_err(error)?;
        }
    }
    Ok(())
}
fn apply(plan_path: &Path) -> Result<(), String> {
    let stage = plan_path
        .parent()
        .ok_or("Invalid update plan")?
        .canonicalize()
        .map_err(error)?;
    no_links(&stage)?;
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
    let guard = installation_guard(&plan.target)?;
    if plan.installer {
        let status = hidden(
            Command::new(package)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"])
                .arg(format!("/DIR={}", shell_path(&plan.target).display()))
                .arg(format!(
                    "/LOG={}",
                    shell_path(&stage.join("installer.log")).display()
                )),
        )
        .status()
        .map_err(error)?;
        if !status.success() {
            return Err(
                "The installer could not finish. Its log is in the update recovery folder.".into(),
            );
        }
    } else {
        let unpacked = extract(&stage, &manifest)?;
        replace(&plan.target, &unpacked, &stage, &manifest, &envelope)?;
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
fn restore_previous(plan: &Plan, stage: &Path) -> Result<(), String> {
    let guard = installation_guard(&plan.target)?;
    let journal: Vec<(String, bool)> =
        serde_json::from_slice(&std::fs::read(stage.join("rollback.json")).map_err(error)?)
            .map_err(error)?;
    rollback(&plan.target, stage, &journal)?;
    drop(guard);
    hidden(Command::new(plan.target.join("AetherDesk.exe")).current_dir(&plan.target))
        .spawn()
        .map_err(error)?;
    Ok(())
}
pub fn mark_healthy(stage: &Path) -> Result<(), String> {
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
    std::fs::write(stage.join("healthy"), b"rendered").map_err(error)
}
pub fn entry(path: &Path) {
    if let Err(message) = apply(path) {
        if let Some(stage) = path.parent() {
            let _ = std::fs::write(stage.join("error.txt"), &message);
        }
        #[cfg(windows)]
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

#[cfg(test)]
mod tests {
    use super::*;
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
