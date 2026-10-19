use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionType {
    SshTerminal,
    RemoteDesktop,
    SshX11,
    SftpOnly,
}
impl ConnectionType {
    pub fn from_name(name: &str) -> Self {
        match name {
            "remoteDesktop" => Self::RemoteDesktop,
            "sshX11" => Self::SshX11,
            "sftpOnly" => Self::SftpOnly,
            _ => Self::SshTerminal,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::SshTerminal => "sshTerminal",
            Self::RemoteDesktop => "remoteDesktop",
            Self::SshX11 => "sshX11",
            Self::SftpOnly => "sftpOnly",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::SshTerminal => "SSH Terminal",
            Self::RemoteDesktop => "Remote Desktop",
            Self::SshX11 => "SSH + X11",
            Self::SftpOnly => "SFTP Only",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedSession {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub connection_type: ConnectionType,
    pub authentication: String,
    pub desktop_environment: String,
    pub key_path: Option<String>,
    pub wireguard_config_path: Option<String>,
    pub wireguard_enabled: bool,
    pub remember_username: bool,
    pub remember_password: bool,
    pub master_password_protected: bool,
    pub favorite: bool,
    pub deleted_at: Option<String>,
}

fn text(map: &Map<String, Value>, name: &str) -> Result<String, String> {
    map.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("Invalid session field: {name}"))
}
fn optional_text(map: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match map.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(format!("Invalid session field: {name}")),
    }
}
fn boolean(map: &Map<String, Value>, name: &str, default: bool) -> Result<bool, String> {
    match map.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(format!("Invalid session field: {name}")),
    }
}
fn enum_name(
    map: &Map<String, Value>,
    key: &str,
    known: &[&str],
    default: &str,
) -> Result<String, String> {
    let name = optional_text(map, key)?.unwrap_or_default();
    Ok(if known.contains(&name.as_str()) {
        name
    } else {
        default.to_owned()
    })
}
impl SavedSession {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let map = value
            .as_object()
            .ok_or("A saved session must be an object")?;
        let username = optional_text(map, "username")?.unwrap_or_default();
        let port = match map.get("port") {
            None | Some(Value::Null) => 22,
            Some(value) => value
                .as_i64()
                .or_else(|| value.as_f64().map(|v| v as i64))
                .ok_or("Invalid session field: port")?,
        };
        let wireguard_config_path =
            optional_text(map, "wireguardConfigPath")?.filter(|p| !p.trim().is_empty());
        let wireguard_enabled = boolean(map, "wireguardEnabled", wireguard_config_path.is_some())?;
        Ok(Self {
            id: text(map, "id")?,
            name: text(map, "name")?,
            host: text(map, "host")?,
            port,
            remember_username: boolean(map, "rememberUsername", !username.is_empty())?,
            username,
            connection_type: ConnectionType::from_name(
                &optional_text(map, "connectionType")?.unwrap_or_default(),
            ),
            authentication: enum_name(
                map,
                "authentication",
                &["password", "sshKey", "smartCard", "mfa"],
                "password",
            )?,
            desktop_environment: enum_name(
                map,
                "desktopEnvironment",
                &["xfce", "kde", "gnome", "mate", "cinnamon", "lxqt"],
                "xfce",
            )?,
            key_path: optional_text(map, "keyPath")?,
            wireguard_config_path,
            wireguard_enabled,
            remember_password: boolean(map, "rememberPassword", false)?,
            master_password_protected: boolean(map, "masterPasswordProtected", false)?,
            favorite: boolean(map, "favorite", false)?,
            deleted_at: optional_text(map, "deletedAt")?.filter(|s| {
                chrono::DateTime::parse_from_rfc3339(s).is_ok()
                    || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
                    || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
            }),
        })
    }
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "version":2, "id":self.id,"name":self.name,"host":self.host,"port":self.port,
            "username":if self.remember_username {self.username.as_str()} else {""},
            "connectionType":self.connection_type.name(),
            "authentication":self.authentication,"desktopEnvironment":self.desktop_environment,
            "rememberUsername":self.remember_username,"rememberPassword":self.remember_password,
            "masterPasswordProtected":self.remember_password && self.master_password_protected,
            "favorite":self.favorite,"deletedAt":self.deleted_at,
        });
        if let Some(key) = self.key_path.as_ref().filter(|s| !s.is_empty()) {
            value["keyPath"] = json!(key);
        }
        if let Some(config) = &self.wireguard_config_path {
            value["wireguardConfigPath"] = json!(config);
            value["wireguardEnabled"] = json!(self.wireguard_enabled);
        }
        value
    }
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
    pub fn ask_every_time(&self) -> bool {
        !self.remember_password
    }
}

pub fn decode_sessions(bytes: &[u8]) -> Result<Vec<SavedSession>, String> {
    let values: Vec<Value> = serde_json::from_slice(bytes).map_err(|_| "Invalid session JSON")?;
    values.iter().map(SavedSession::from_json).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wireguard_enablement_is_per_profile_and_disabled_files_survive_reload() {
        let enabled=SavedSession::from_json(&json!({"id":"one","name":"VPN","host":"server","wireguardConfigPath":"C:/vpn/peer.conf"})).unwrap();
        assert!(enabled.wireguard_enabled); // old profiles retain their choice
        let mut disabled = enabled.clone();
        disabled.id = "two".into();
        disabled.wireguard_enabled = false;
        let restored = SavedSession::from_json(&disabled.to_json()).unwrap();
        assert!(!restored.wireguard_enabled);
        assert_eq!(
            restored.wireguard_config_path,
            enabled.wireguard_config_path
        );
        assert!(enabled.wireguard_enabled);
        disabled.wireguard_config_path = None;
        assert!(
            !SavedSession::from_json(&disabled.to_json())
                .unwrap()
                .wireguard_enabled
        );
    }
    #[test]
    fn username_and_master_password_consent_are_preserved() {
        let mut session = SavedSession::from_json(&json!({
            "id":"one","name":"Work","host":"server","username":"reza",
            "rememberUsername":false,"rememberPassword":false,"masterPasswordProtected":true
        }))
        .unwrap();
        let encoded = session.to_json();
        assert_eq!(encoded["username"], "");
        assert_eq!(encoded["masterPasswordProtected"], false);
        assert!(encoded.get("password").is_none());
        session.remember_password = true;
        assert_eq!(session.to_json()["masterPasswordProtected"], true);
    }
    #[test]
    fn unknown_old_profile_choices_keep_legacy_fallbacks() {
        let session = SavedSession::from_json(&json!({
            "id":"one","name":"Work","host":"server","username":"reza",
            "connectionType":"future","authentication":"future","desktopEnvironment":"future"
        }))
        .unwrap();
        assert_eq!(session.connection_type, ConnectionType::SshTerminal);
        assert_eq!(session.authentication, "password");
        assert_eq!(session.desktop_environment, "xfce");
        assert!(session.remember_username);
        assert_eq!(session.port, 22);
    }
}
