//! Persist duplicate desktop identities before any remote launch. These are
//! recovery bookmarks, not a claim that the host still has a running desktop.
use ad_desktop_model::profiles::{ConnectionType, SavedSession};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub desktop_id: Uuid,
    pub profile_id: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub account_hash: String,
    pub name: String,
}
fn account_hash(profile: &SavedSession, username: &str) -> String {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    context.update(profile.id.as_bytes());
    context.update(&[0]);
    context.update(username.as_bytes());
    context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn entry(profile: &SavedSession, username: &str, desktop_id: Uuid) -> Result<Entry, String> {
    if username.is_empty() || username.len() > 256 || username.chars().any(char::is_control) {
        return Err("Enter a valid username before saving a desktop recovery entry".into());
    }
    Ok(Entry {
        desktop_id,
        profile_id: profile.id.clone(),
        host: profile.host.trim().into(),
        port: profile.port,
        username: if profile.remember_username {
            username.into()
        } else {
            String::new()
        },
        account_hash: account_hash(profile, username),
        name: profile.name.clone(),
    })
}
impl Entry {
    pub fn matches(&self, profile: &SavedSession) -> bool {
        !profile.is_deleted()
            && profile.connection_type == ConnectionType::RemoteDesktop
            && self.profile_id == profile.id
            && self.host == profile.host.trim()
            && self.port == profile.port
    }
}
fn decode(value: serde_json::Value) -> Result<Vec<Entry>, String> {
    let entries: Vec<Entry> = serde_json::from_value(value).map_err(|_| {
        "Desktop recovery records are damaged; the file has been preserved".to_owned()
    })?;
    let mut ids = std::collections::HashSet::new();
    if entries.len() > 4096
        || entries.iter().any(|e| {
            e.desktop_id.is_nil()
                || !ids.insert(e.desktop_id)
                || e.profile_id.is_empty()
                || e.profile_id.len() > 256
                || e.host.is_empty()
                || e.host.len() > 253
                || e.host.chars().any(char::is_control)
                || !(1..=65535).contains(&e.port)
                || e.username.len() > 256
                || e.username.chars().any(char::is_control)
                || e.account_hash.len() != 64
                || !e.account_hash.bytes().all(|b| b.is_ascii_hexdigit())
                || e.name.len() > 256
        })
    {
        return Err("Invalid desktop recovery records; the file has been preserved".into());
    }
    Ok(entries)
}
pub fn load() -> Result<Vec<Entry>, String> {
    decode(
        crate::storage::read_json("desktop-recovery.json")?
            .unwrap_or_else(|| serde_json::json!([])),
    )
}
fn save(entries: &[Entry]) -> Result<(), String> {
    let value = serde_json::to_value(entries).map_err(|e| e.to_string())?;
    decode(value.clone())?;
    crate::storage::write_json("desktop-recovery.json", &value)
}
pub fn remember(profile: &SavedSession, username: &str, desktop_id: Uuid) -> Result<(), String> {
    let mut entries = load()?;
    let entry = entry(profile, username, desktop_id)?;
    if let Some(existing) = entries.iter_mut().find(|e| e.desktop_id == desktop_id) {
        if !existing.matches(profile) || existing.account_hash != entry.account_hash {
            return Err("This desktop recovery identity belongs to a different connection".into());
        }
        *existing = entry;
    } else {
        entries.push(entry);
    }
    save(&entries)
}
pub fn forget(desktop_id: Uuid) -> Result<(), String> {
    let mut entries = load()?;
    entries.retain(|e| e.desktop_id != desktop_id);
    save(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> SavedSession {
        SavedSession::from_json(&serde_json::json!({"id":"p","name":"Test","host":"server","port":22,"username":"alice","connectionType":"remoteDesktop"})).unwrap()
    }
    #[test]
    fn recovery_round_trip_preserves_independent_ids_and_username() {
        let p = profile();
        let entries = vec![Entry {
            desktop_id: Uuid::new_v4(),
            profile_id: p.id.clone(),
            host: p.host.clone(),
            port: p.port,
            username: "alice".into(),
            account_hash: account_hash(&p, "alice"),
            name: "Duplicate".into(),
        }];
        let decoded = decode(serde_json::to_value(&entries).unwrap()).unwrap();
        assert_eq!(entries[0].desktop_id, decoded[0].desktop_id);
        assert_eq!(decoded[0].username, "alice");
        assert!(decoded[0].matches(&p));
        let mut changed = p.clone();
        changed.host = "another-host".into();
        assert!(!decoded[0].matches(&changed));
        changed = p;
        changed.port = 2222;
        assert!(!decoded[0].matches(&changed));
    }
    #[test]
    fn duplicate_or_corrupt_records_are_rejected() {
        let p = profile();
        let entry = entry(&p, "alice", Uuid::new_v4()).unwrap();
        assert!(decode(serde_json::to_value([entry.clone(), entry]).unwrap()).is_err());
        assert!(decode(serde_json::json!([{ "desktopId":"not-a-uuid" }])).is_err());
    }
    #[test]
    fn recovery_respects_username_consent_and_binds_the_account() {
        let mut p = profile();
        p.remember_username = false;
        let record = entry(&p, "alice", Uuid::new_v4()).unwrap();
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("alice"));
        assert!(record.username.is_empty());
        assert_eq!(record.account_hash, account_hash(&p, "alice"));
        assert_ne!(record.account_hash, account_hash(&p, "bob"));
        assert_eq!(
            decode(serde_json::to_value([record]).unwrap())
                .unwrap()
                .len(),
            1
        );
    }
}
