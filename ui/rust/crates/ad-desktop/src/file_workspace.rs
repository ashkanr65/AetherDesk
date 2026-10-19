//! File actions and editor ownership checks. No path is passed to a shell.
use crate::editor::Location;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

struct Pin {
    local: Option<PathBuf>,
    host: Option<String>,
    remote: Option<String>,
}
fn pins() -> &'static Mutex<HashMap<Uuid, Pin>> {
    static PINS: OnceLock<Mutex<HashMap<Uuid, Pin>>> = OnceLock::new();
    PINS.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn protect(id: Uuid, location: &Location, canonical: Option<&str>) {
    let pin = match location {
        Location::Local(path) => Pin {
            local: Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone())),
            host: None,
            remote: None,
        },
        Location::Remote { session, .. } => Pin {
            local: None,
            host: Some(crate::working_copies::identity(session)),
            remote: canonical.map(str::to_owned),
        },
    };
    pins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, pin);
}
pub fn release(id: Uuid) {
    pins().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}
fn inside(path: &str, parent: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/'))
}
fn local_key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    {
        value.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}
pub fn check_local(path: &Path) -> Result<(), String> {
    let canonical = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::canonicalize(path.parent().ok_or("Invalid local parent")?)
                .map_err(|e| e.to_string())?
                .join(path.file_name().ok_or("Invalid filename")?)
        }
        Err(e) => return Err(e.to_string()),
    };
    let key = local_key(&canonical);
    if pins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .any(|p| {
            p.local
                .as_ref()
                .is_some_and(|p| inside(&local_key(p), &key))
        })
    {
        return Err(
            "Close the open document before renaming or deleting it or its containing folder"
                .into(),
        );
    }
    Ok(())
}
pub fn check_remote(session: &ad_ssh::SshSession, path: &str) -> Result<(), String> {
    let host = crate::working_copies::identity(session);
    if pins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .any(|p| {
            p.host.as_ref() == Some(&host) && p.remote.as_ref().is_none_or(|p| inside(p, path))
        })
    {
        return Err("Close the open remote document before renaming or deleting it or its containing folder".into());
    }
    Ok(())
}
fn regular_target(path: &Path) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("A drive root cannot be changed")?;
    let parent = path.parent().ok_or("Choose a file or folder")?;
    let parent = std::fs::canonicalize(parent).map_err(|e| e.to_string())?;
    let target = parent.join(name);
    if target.parent() != Some(parent.as_path()) {
        return Err("Invalid file target".into());
    }
    let metadata = std::fs::symlink_metadata(&target).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("This action does not follow Windows links or junctions".into());
        }
    }
    if metadata.file_type().is_symlink() {
        return Err("This action does not follow symbolic links".into());
    }
    Ok(target)
}
pub fn local_action(
    operation: &str,
    path: PathBuf,
    value: String,
) -> Result<Option<String>, String> {
    if operation == "mkdir" {
        let target = PathBuf::from(value);
        let name = target.file_name().ok_or("Choose a folder name")?;
        let parent = std::fs::canonicalize(target.parent().ok_or("Folder parent missing")?)
            .map_err(|e| e.to_string())?;
        std::fs::create_dir(parent.join(name)).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    let target = regular_target(&path)?;
    match operation {
        "rename" => {
            check_local(&target)?;
            let destination = PathBuf::from(value);
            let name = destination.file_name().ok_or("Choose a new filename")?;
            let parent =
                std::fs::canonicalize(destination.parent().ok_or("Destination parent missing")?)
                    .map_err(|e| e.to_string())?;
            let destination = parent.join(name);
            if destination.exists() {
                return Err("Destination already exists; it was not replaced".into());
            }
            ad_ssh::rename_local_noclobber(&target, &destination).map_err(|e| e.to_string())?;
        }
        "delete" => {
            check_local(&target)?;
            if target.is_dir() {
                verify_tree(&target, 0, &mut 0)?;
                std::fs::remove_dir_all(target)
            } else {
                std::fs::remove_file(target)
            }
            .map_err(|e| e.to_string())?;
        }
        "readonly" => {
            let mut permissions = std::fs::metadata(&target)
                .map_err(|e| e.to_string())?
                .permissions();
            permissions.set_readonly(value == "true");
            std::fs::set_permissions(target, permissions).map_err(|e| e.to_string())?;
        }
        "stat" => {
            let metadata = std::fs::metadata(&target).map_err(|e| e.to_string())?;
            return Ok(Some(format!(
                "Path: {}\nType: {}\nSize: {} bytes\nModified: {:?}\nCreated: {:?}\nRead-only: {}",
                target.display(),
                if metadata.is_dir() { "folder" } else { "file" },
                metadata.len(),
                metadata.modified().ok(),
                metadata.created().ok(),
                metadata.permissions().readonly()
            )));
        }
        _ => return Err("Unknown local file operation".into()),
    }
    Ok(None)
}
fn verify_tree(path: &Path, depth: usize, count: &mut usize) -> Result<(), String> {
    if depth > 128 || *count > 100000 {
        return Err("Folder exceeds the safe traversal limit".into());
    }
    regular_target(path)?;
    for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        *count += 1;
        let target = regular_target(&entry.path())?;
        if target.is_dir() {
            verify_tree(&target, depth + 1, count)?;
        }
    }
    Ok(())
}
pub fn open_with(path: &Path) -> Result<bool, String> {
    crate::editor::check_external_file(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::Shell::{SHOpenWithDialog, OAIF_EXEC, OPENASINFO},
        };
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let initialized =
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if initialized < 0 {
            return Err("The Windows application picker could not initialize".into());
        }
        let info = OPENASINFO {
            pcszFile: path.as_ptr(),
            pcszClass: std::ptr::null(),
            oaifInFlags: OAIF_EXEC,
        };
        let result = unsafe { SHOpenWithDialog(std::ptr::null_mut(), &info) };
        unsafe { CoUninitialize() };
        if result == 0 {
            Ok(true)
        } else if result as u32 == 0x800704c7 {
            Ok(false)
        } else {
            Err(format!("Windows application picker failed ({result:#x})"))
        }
    }
    #[cfg(not(windows))]
    {
        Err("Application selection is available on Windows".into())
    }
}
pub fn properties(path: &Path, security: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
            UI::Shell::{SHObjectProperties, SHOP_FILEPATH},
        };
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let tab: Vec<u16> = "Security".encode_utf16().chain(Some(0)).collect();
        let initialized =
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if initialized < 0 {
            return Err("Windows properties could not initialize".into());
        }
        let result = unsafe {
            SHObjectProperties(
                std::ptr::null_mut(),
                SHOP_FILEPATH as u32,
                path.as_ptr(),
                if security {
                    tab.as_ptr()
                } else {
                    std::ptr::null()
                },
            )
        };
        unsafe { CoUninitialize() };
        if result != 0 {
            Ok(())
        } else {
            Err("Windows properties could not be opened".into())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, security);
        Err("Windows properties are unavailable".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_open_document_blocks_parent_rename_without_blocking_similar_names() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("folder");
        std::fs::create_dir(&dir).unwrap();
        let doc = dir.join("a.txt");
        std::fs::write(&doc, b"a").unwrap();
        let id = Uuid::new_v4();
        protect(id, &Location::Local(doc), None);
        assert!(check_local(&dir).is_err());
        let other = root.path().join("folder-more");
        std::fs::create_dir(&other).unwrap();
        assert!(check_local(&other).is_ok());
        release(id);
        assert!(check_local(&dir).is_ok());
    }
    #[test]
    fn local_rename_never_replaces_an_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        assert!(local_action("rename", a.clone(), b.to_string_lossy().into()).is_err());
        assert_eq!(std::fs::read(a).unwrap(), b"a");
        assert_eq!(std::fs::read(b).unwrap(), b"b");
    }
    #[test]
    fn new_download_destination_is_allowed_without_ignoring_parent_errors() {
        let root = tempfile::tempdir().unwrap();
        assert!(check_local(&root.path().join("new.txt")).is_ok());
        assert!(check_local(&root.path().join("missing/new.txt")).is_err());
    }
}
