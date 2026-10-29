//! Separate native profile store; production files are only read on import.
use ad_desktop_model::profiles::{decode_sessions, SavedSession};
use serde_json::{json, Value};

pub fn load() -> Result<Vec<SavedSession>, String> {
    match crate::storage::read_json("sessions.json")? {
        Some(value) => decode_sessions(&serde_json::to_vec(&value).map_err(|e| e.to_string())?),
        None => {
            let Some(backup) = crate::storage::read_json("sessions.json.bak")? else {
                return Ok(Vec::new());
            };
            let profiles =
                decode_sessions(&serde_json::to_vec(&backup).map_err(|e| e.to_string())?)?;
            // Validate before restoring; preserve the recovery file as evidence.
            crate::storage::write_json("sessions.json", &backup)?;
            Ok(profiles)
        }
    }
}
pub fn save(profiles: &[SavedSession]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for profile in profiles {
        if !ids.insert(&profile.id) {
            return Err("Duplicate saved connection identity".into());
        }
    }
    crate::storage::write_json(
        "sessions.json",
        &Value::Array(profiles.iter().map(SavedSession::to_json).collect()),
    )
}
/// Merge only the credential preferences of the same saved connection. A
/// password choice made in Connect must also survive opening a fresh tab.
pub fn credential_update(profiles: &mut [SavedSession], update: SavedSession) -> bool {
    let Some(saved) = profiles.iter_mut().find(|p| {
        p.id == update.id
            && p.host == update.host
            && p.port == update.port
            && p.authentication == update.authentication
            && p.key_path == update.key_path
    }) else {
        return false;
    };
    if saved.username == update.username
        && saved.remember_username == update.remember_username
        && saved.remember_password == update.remember_password
        && saved.master_password_protected == update.master_password_protected
    {
        return false;
    }
    saved.username = update.username;
    saved.remember_username = update.remember_username;
    saved.remember_password = update.remember_password;
    saved.master_password_protected = update.master_password_protected;
    true
}
pub fn validate(profile: &SavedSession) -> Result<(), String> {
    if profile.name.trim().is_empty() || profile.name.len() > 200 {
        return Err("Enter a connection name up to 200 characters".into());
    }
    if profile.host.trim().is_empty()
        || profile.host.len() > 253
        || profile.host.chars().any(char::is_control)
    {
        return Err("Enter a valid server address".into());
    }
    if !(1..=65535).contains(&profile.port) {
        return Err("Port must be between 1 and 65535".into());
    }
    Ok(())
}
pub fn new_profile() -> SavedSession {
    let preferences = crate::preferences::current();
    SavedSession::from_json(&json!({"id":uuid::Uuid::new_v4().to_string(),"name":"New connection","host":"","username":"","port":preferences.default_port,"rememberUsername":preferences.remember_username})).expect("literal session defaults")
}
pub fn import(path: &std::path::Path) -> Result<Vec<SavedSession>, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        return Err("Import a session JSON file up to 8 MiB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut profiles = decode_sessions(&bytes)?;
    for p in &mut profiles {
        // Imported automation needs a deliberate opt-in on this installation.
        p.startup_enabled = false;
        // Credentials belong to their original app and are never silently reused.
        p.id = uuid::Uuid::new_v4().to_string();
        p.remember_password = false;
        p.master_password_protected = false;
    }
    Ok(profiles)
}
pub fn export(path: &std::path::Path, profiles: &[SavedSession]) -> Result<(), String> {
    use std::io::Write;
    let value = Value::Array(
        profiles
            .iter()
            .map(|p| {
                let mut v = p.to_json();
                v["rememberPassword"] = json!(false);
                v["masterPasswordProtected"] = json!(false);
                v
            })
            .collect(),
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_consent_updates_only_the_bound_profile_and_preserves_its_settings() {
        let mut saved = new_profile();
        saved.host = "fixture.invalid".into();
        saved.favorite = true;
        let mut profiles = vec![saved.clone()];
        let mut update = saved.clone();
        update.username = "fixture-user".into();
        update.remember_password = true;
        update.remember_username = true;
        update.name = "must not replace the saved name".into();
        assert!(credential_update(&mut profiles, update.clone()));
        assert!(profiles[0].favorite);
        assert_eq!(profiles[0].name, saved.name);
        assert!(profiles[0].remember_password);
        assert_eq!(profiles[0].username, "fixture-user");
        assert!(!credential_update(&mut profiles, update.clone()));
        update.host = "other.invalid".into();
        update.remember_password = false;
        assert!(!credential_update(&mut profiles, update));
        assert!(profiles[0].remember_password);
    }
    #[test]
    fn a_valid_backup_recovers_a_missing_store_but_never_masks_a_corrupt_primary() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let backup = json!([{"id":"recovered","name":"saved","host":"server"}]);
            crate::storage::write_json("sessions.json.bak", &backup).unwrap();
            assert_eq!(load().unwrap()[0].id, "recovered");
            assert!(dir.path().join("sessions.json").is_file());
            std::fs::write(dir.path().join("sessions.json"), b"damaged").unwrap();
            assert!(load().is_err());
            assert_eq!(
                std::fs::read(dir.path().join("sessions.json")).unwrap(),
                b"damaged"
            );
            assert_eq!(
                crate::storage::read_json("sessions.json.bak")
                    .unwrap()
                    .unwrap(),
                backup
            );
        });
    }
    #[test]
    fn profile_import_export_preserves_consent_and_never_overwrites_or_reuses_credentials() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let profile=SavedSession::from_json(&json!({"id":"original","name":"test","host":"server","username":"private","rememberUsername":false,"rememberPassword":true,"masterPasswordProtected":true,"deletedAt":"invalid-date","connectionType":"sshX11"})).unwrap();
            save(std::slice::from_ref(&profile)).unwrap();
            let loaded = load().unwrap();
            assert!(loaded[0].username.is_empty());
            assert!(loaded[0].deleted_at.is_none());
            assert!(save(&[profile.clone(), profile.clone()]).is_err());
            let path = dir.path().join("export.json");
            export(&path, std::slice::from_ref(&profile)).unwrap();
            let original = std::fs::read(&path).unwrap();
            assert!(export(&path, &[]).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
            let imported = import(&path).unwrap();
            assert_ne!(imported[0].id, profile.id);
            assert!(!imported[0].remember_password);
            assert!(!imported[0].master_password_protected);
            assert!(!String::from_utf8(original).unwrap().contains("private"));
        });
    }
}
