//! Explicit, read-only import from the Flutter AetherDesk data directories.
use crate::{
    assistant::{Chat, Provider},
    credentials, terminal_settings, vault,
};
use ad_desktop_model::profiles::{decode_sessions, SavedSession};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use zeroize::Zeroizing;

fn appdata() -> Result<std::path::PathBuf, String> {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "APPDATA is unavailable".into())
}
fn localdata() -> Result<std::path::PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is unavailable".into())
}
fn read_json(path: &std::path::Path) -> Result<Option<Value>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Could not inspect original data: {e}")),
    };
    if metadata.len() > 32 * 1024 * 1024 {
        return Err("Original data file exceeds 32 MiB; it was left untouched".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read original data: {e}"))?;
    serde_json::from_slice(&bytes).map(Some).map_err(|_| {
        format!(
            "Original data file is damaged and was left untouched: {}",
            path.display()
        )
    })
}
pub(crate) fn import_profiles(
    include_credentials: bool,
    master: &str,
) -> Result<(Vec<SavedSession>, usize, usize), String> {
    let path = appdata()?.join("AetherDesk").join("sessions.json");
    let Some(value) = read_json(&path)? else {
        return Ok((Vec::new(), 0, 0));
    };
    let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    let mut profiles = decode_sessions(&bytes)?;
    let mut copied = 0;
    let mut skipped = 0;
    let existing = crate::profile_store::load()?
        .into_iter()
        .map(|p| p.id)
        .collect::<std::collections::HashSet<_>>();
    let mut seen = existing;
    for profile in &mut profiles {
        let legacy_id = profile.id.clone();
        if profile.id.is_empty() || profile.id.len() > 180 || !seen.insert(profile.id.clone()) {
            profile.id = uuid::Uuid::new_v4().to_string();
            seen.insert(profile.id.clone());
        }
        if include_credentials && profile.remember_password {
            match read_secret(&format!("AetherDesk/{legacy_id}"), master) {
                Ok(Some((username, secret))) => {
                    let user = if username.is_empty() {
                        profile.username.as_str()
                    } else {
                        username.as_str()
                    };
                    if !profile.master_password_protected || !master.is_empty() {
                        if credentials::remember(profile, user, &secret, master).is_ok() {
                            copied += 1
                        } else {
                            skipped += 1;
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => skipped += 1,
            }
        }
    }
    Ok((profiles, copied, skipped))
}
pub(crate) struct AssistantImport {
    pub providers: Vec<Provider>,
    pub chats: Vec<Chat>,
    pub credentials: usize,
    pub skipped_credentials: usize,
    pub terminal: bool,
}
pub(crate) fn import_assistant(
    include_credentials: bool,
    master: &str,
) -> Result<AssistantImport, String> {
    let root = localdata()?.join("AetherDesk");
    let mut terminal = false;
    let terminal_path = root.join("terminal-theme.json");
    if let Some(value) = read_json(&terminal_path)? {
        if crate::storage::read_json("terminal-settings.json")?.is_none() {
            let mut mapped = json!({"preset":value["preset"].as_str().unwrap_or("mobaXterm"),"font":"Bundled monospace","fontSize":value["fontSize"].as_f64().unwrap_or(13.0),"lineHeight":value["lineHeight"].as_f64().unwrap_or(1.2),"scrollbackLines":value["scrollbackLines"].as_u64().unwrap_or(100_000),"autoClassify":value["autoClassify"].as_bool().unwrap_or(true),"followCwd":true,"semanticColors":value["semanticColors"].as_object().cloned().unwrap_or_default()});
            if let Some(colors) = mapped["semanticColors"].as_object_mut() {
                colors.retain(|k, v| {
                    terminal_settings::ROLES.contains(&k.as_str())
                        && v.as_u64().is_some_and(|n| n <= u32::MAX as u64)
                });
            }
            let settings: terminal_settings::Settings = serde_json::from_value(mapped)
                .map_err(|_| "Original terminal preferences are invalid")?;
            terminal_settings::save(settings)?;
            terminal = true;
        }
    }
    let assistant = root.join("assistant");
    let settings = read_json(&assistant.join("settings.json"))?.unwrap_or_else(|| json!({}));
    let provider_settings = settings["providerSettings"].as_object();
    let mut ids = std::collections::BTreeSet::new();
    if let Some(values) = settings["providers"].as_array() {
        for id in values.iter().filter_map(Value::as_str) {
            if id.len() < 160
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                ids.insert(id.to_owned());
            }
        }
    }
    if let Some(map) = provider_settings {
        for id in map.keys() {
            if id.len() < 160
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                ids.insert(id.clone());
            }
        }
    }
    let mut providers = Vec::new();
    let mut credentials_copied = 0;
    let mut credentials_skipped = 0;
    for id in ids {
        let setting = provider_settings
            .and_then(|m| m.get(&id))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let name = match id.as_str() {
            "openrouter" => "OpenRouter",
            "nvidia" => "NVIDIA",
            "groq" => "Groq",
            "ollama" => "Ollama",
            "openai" => "OpenAI",
            _ => id.as_str(),
        };
        let base = setting["baseURL"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| match id.as_str() {
                "openrouter" => "https://openrouter.ai/api/v1".into(),
                "nvidia" => "https://integrate.api.nvidia.com/v1".into(),
                "groq" => "https://api.groq.com/openai/v1".into(),
                "ollama" => "http://127.0.0.1:11434/v1".into(),
                "openai" => "https://api.openai.com/v1".into(),
                _ => String::new(),
            });
        if base.is_empty() {
            continue;
        }
        let mut provider = Provider {
            id: id.clone(),
            name: name.into(),
            base,
            models: Vec::new(),
            capabilities: Default::default(),
            manual_models: Vec::new(),
        };
        if let Some(model) = setting["modelId"].as_str().filter(|s| !s.is_empty()) {
            provider.models.push(model.into());
            provider.manual_models.push(model.into());
            provider
                .capabilities
                .insert(model.into(), crate::ai_models::Model::manual(model));
        }
        if include_credentials {
            let target = format!("AetherDesk/assistant/provider/{id}");
            match read_secret(&target, master) {
                Ok(Some((_, secret))) => {
                    if vault::read(&id)?.is_none() {
                        if vault::write(&id, &secret).is_ok() {
                            credentials_copied += 1
                        } else {
                            credentials_skipped += 1;
                        }
                    } else {
                        credentials_skipped += 1;
                    }
                }
                Ok(None) => {}
                Err(_) => credentials_skipped += 1,
            }
        }
        providers.push(provider);
    }
    let history_path = assistant.join("langgraph-sessions.json");
    let mut chats = read_json(&history_path)?
        .map(|value| convert_history(&value))
        .transpose()?
        .unwrap_or_default();
    if let Some(selected) = settings["model"].as_str() {
        if let Some((provider, model)) = selected.split_once('/') {
            if let Some(chat) = chats.first_mut() {
                chat.provider_id = provider.into();
                chat.model = model.into();
            }
        }
    }
    if let Some(path) = settings["workspace"].as_str() {
        if let Ok(path) = std::fs::canonicalize(path) {
            if path.is_dir() {
                if let Some(chat) = chats.first_mut() {
                    chat.workspace_id = format!("local:{}", path.display());
                    chat.workspace_label = path.display().to_string();
                }
            }
        }
    }
    Ok(AssistantImport {
        providers,
        chats,
        credentials: credentials_copied,
        skipped_credentials: credentials_skipped,
        terminal,
    })
}
fn convert_history(value: &Value) -> Result<Vec<Chat>, String> {
    let sessions = value["sessions"]
        .as_array()
        .ok_or("Original AI chat history has no sessions array")?;
    if sessions.len() > 10_000 {
        return Err("Original AI history contains too many conversations".into());
    }
    let mut chats = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for session in sessions {
        let mut id = session["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 300 && !s.chars().any(char::is_control))
            .unwrap_or("legacy-chat")
            .to_owned();
        if !seen.insert(id.clone()) {
            id = uuid::Uuid::new_v4().to_string();
            seen.insert(id.clone());
        }
        let mut messages = Vec::new();
        if let Some(source) = session["messages"].as_array() {
            for message in source.iter().take(100_000) {
                let role = message["info"]["role"].as_str().unwrap_or("");
                if !matches!(role, "user" | "assistant") {
                    continue;
                }
                let mut text = String::new();
                let mut image_count = 0;
                if let Some(parts) = message["parts"].as_array() {
                    for part in parts {
                        if let Some(s) = part["text"].as_str() {
                            if text.len() < 2 * 1024 * 1024 {
                                text.push_str(s);
                            }
                        } else if part["type"] == "image" {
                            image_count += 1;
                        }
                    }
                }
                if image_count > 0 {
                    text.push_str(&format!("\n[Imported transcript had {image_count} image attachment(s); original image data is not replayed.]"));
                }
                if !text.is_empty() {
                    messages.push(json!({"role":role,"content":text}));
                }
            }
        }
        chats.push(Chat {
            id,
            title: session["title"]
                .as_str()
                .unwrap_or("Imported conversation")
                .chars()
                .take(200)
                .collect(),
            messages,
            workspace_id: String::new(),
            workspace_label: String::new(),
            provider_id: String::new(),
            model: String::new(),
            draft: String::new(),
            context_tokens: 16_384,
            output_tokens: 2_048,
            use_tools: false,
            reasoning: None,
            reasoning_tokens: 0,
            nvidia_thinking: false,
            loaded: true,
        });
    }
    Ok(chats)
}
pub(crate) fn read_secret(
    target: &str,
    master: &str,
) -> Result<Option<(String, Zeroizing<String>)>, String> {
    let Some((username, blob)) = vault::read_legacy(target)? else {
        return Ok(None);
    };
    let envelope: Value =
        serde_json::from_slice(&blob).map_err(|_| "Legacy credential envelope is invalid")?;
    if envelope["version"] != 2 {
        return Err("Unsupported legacy credential version".into());
    }
    let protected = envelope["masterPasswordProtected"]
        .as_bool()
        .unwrap_or(false);
    let salt = STANDARD
        .decode(envelope["salt"].as_str().unwrap_or(""))
        .map_err(|_| "Invalid legacy credential salt")?;
    let data = STANDARD
        .decode(envelope["protectedData"].as_str().unwrap_or(""))
        .map_err(|_| "Invalid legacy credential data")?;
    let protection = envelope["protection"].as_str().unwrap_or("dpapi");
    let plaintext = match protection {
        "windowsCredentialManager" if !protected => Zeroizing::new(data),
        "dpapi" => {
            let entropy = if protected {
                if master.is_empty() {
                    return Err("This legacy credential needs its master password".into());
                }
                let iterations = envelope["iterations"].as_u64().unwrap_or(0);
                if !(100_000..=2_000_000).contains(&iterations) {
                    return Err("Unsupported legacy master-password settings".into());
                }
                let mut derived = Zeroizing::new([0u8; 32]);
                ring::pbkdf2::derive(
                    ring::pbkdf2::PBKDF2_HMAC_SHA256,
                    std::num::NonZeroU32::new(iterations as u32).unwrap(),
                    &salt,
                    master.as_bytes(),
                    derived.as_mut(),
                );
                derived.to_vec()
            } else {
                Vec::new()
            };
            credentials::unprotect_legacy(&data, &entropy)?
        }
        _ => return Err("Legacy credential uses unsupported protection".into()),
    };
    let secret =
        String::from_utf8(plaintext.to_vec()).map_err(|_| "Legacy credential is not UTF-8")?;
    if secret.len() > 5120 {
        return Err("Legacy credential is too large".into());
    }
    Ok(Some((username, Zeroizing::new(secret))))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_transcripts_cannot_replay_tools_permissions_or_images() {
        let input = json!({"sessions":[{"id":"old","title":"imported","messages":[
            {"info":{"role":"user"},"parts":[{"type":"text","text":"hello"},{"type":"image","url":"secret-image"}]},
            {"info":{"role":"assistant"},"parts":[{"type":"tool","state":{"input":{"command":"erase"}}},{"type":"text","text":"reply"}]},
            {"info":{"role":"tool"},"parts":[{"text":"old command result"}]}]},
            {"id":"old","messages":[]} ]});
        let chats = convert_history(&input).unwrap();
        assert_ne!(chats[0].id, chats[1].id);
        assert!(!chats[0].use_tools);
        assert!(chats[0].workspace_id.is_empty());
        assert_eq!(chats[0].messages.len(), 2);
        let text = serde_json::to_string(&chats).unwrap();
        assert!(!text.contains("secret-image"));
        assert!(!text.contains("erase"));
        assert!(!text.contains("old command result"));
    }
}
