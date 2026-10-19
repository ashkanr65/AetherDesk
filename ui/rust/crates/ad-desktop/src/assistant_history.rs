//! Bounded per-chat history files with a small searchable metadata index.
use crate::assistant::Chat;
use serde_json::Value;

fn file_name(id: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, id.as_bytes());
    let hex = digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("ai-chat-{hex}.json")
}
fn validate_chats(chats: &[Chat]) -> Result<(), String> {
    if chats.len() > 10_000 {
        return Err("AI history contains too many chats".into());
    }
    let mut ids = std::collections::HashSet::new();
    if chats
        .iter()
        .any(|c| c.id.is_empty() || c.id.len() > 300 || !ids.insert(&c.id))
    {
        return Err("AI history has invalid chat identities".into());
    }
    Ok(())
}
pub fn load_index() -> Result<Option<Vec<Chat>>, String> {
    let Some(value) = crate::storage::read_json("ai-history-index.json")? else {
        return Ok(None);
    };
    let chats: Vec<Chat> = serde_json::from_value(value)
        .map_err(|_| "AI history index is invalid; its file has been preserved".to_owned())?;
    validate_chats(&chats)?;
    Ok(Some(chats))
}
pub fn load_chat(id: &str) -> Result<Vec<Value>, String> {
    let Some(value) = crate::storage::read_json(&file_name(id))? else {
        return Ok(Vec::new());
    };
    value["messages"]
        .as_array()
        .cloned()
        .ok_or_else(|| "Saved chat messages are damaged; file preserved".into())
}
pub fn save_chat(chat: &Chat) -> Result<(), String> {
    validate_chats(std::slice::from_ref(chat))?;
    if chat.messages.len() > 100_000 {
        return Err("This chat contains too many messages".into());
    }
    crate::storage::write_json(
        &file_name(&chat.id),
        &serde_json::json!({"messages":chat.messages}),
    )
}
pub fn delete_chat(id: &str) -> Result<(), String> {
    let path = crate::storage::root()?.join(file_name(id));
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
pub fn save_index(chats: &[Chat]) -> Result<(), String> {
    validate_chats(chats)?;
    let index=chats.iter().map(|c|serde_json::json!({
        "id":c.id,"title":c.title,"workspace_id":c.workspace_id,"workspace_label":c.workspace_label,
        "provider_id":c.provider_id,"model":c.model,"draft":c.draft,
        "context_tokens":c.context_tokens,"output_tokens":c.output_tokens,"use_tools":c.use_tools,
        "reasoning":c.reasoning,"reasoning_tokens":c.reasoning_tokens,"nvidia_thinking":c.nvidia_thinking
    })).collect::<Vec<_>>();
    crate::storage::write_json("ai-history-index.json", &serde_json::json!(index))
}
pub fn legacy() -> Result<Vec<Chat>, String> {
    let Some(value) = crate::storage::read_json("ai-history.json")? else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value)
        .map_err(|_| "Old AI history is invalid; its file has been preserved".into())
}
pub fn import_chats(chats: &[Chat]) -> Result<(), String> {
    validate_chats(chats)?;
    for chat in chats {
        save_chat(chat)?;
    }
    save_index(chats)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_round_trip_is_lazy_scoped_and_delete_removes_stored_images() {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::with_test_root(dir.path(), || {
            let chat:Chat=serde_json::from_value(serde_json::json!({"id":"../../unsafe-id","title":"one","provider_id":"custom","model":"provider/model","workspace_id":"ssh:first:/work","draft":"unsent","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,image-fixture"}}]}]})).unwrap();
            import_chats(std::slice::from_ref(&chat)).unwrap();
            let index = load_index().unwrap().unwrap();
            assert_eq!(index[0].provider_id, "custom");
            assert_eq!(index[0].workspace_id, "ssh:first:/work");
            assert_eq!(index[0].draft, "unsent");
            assert!(index[0].messages.is_empty());
            assert!(!index[0].loaded);
            assert_eq!(load_chat(&chat.id).unwrap(), chat.messages);
            assert!(
                !std::fs::read_to_string(dir.path().join("ai-history-index.json"))
                    .unwrap()
                    .contains("image-fixture")
            );
            let saved = std::fs::read(dir.path().join(file_name(&chat.id))).unwrap();
            assert!(save_index(&[chat.clone(), chat.clone()]).is_err());
            assert_eq!(
                std::fs::read(dir.path().join(file_name(&chat.id))).unwrap(),
                saved
            );
            delete_chat(&chat.id).unwrap();
            delete_chat(&chat.id).unwrap();
            assert!(!dir.path().join(file_name(&chat.id)).exists());
            save_index(&[]).unwrap();
            assert!(load_index().unwrap().unwrap().is_empty());
        });
    }
}
