//! In-memory action grants. A message grant cannot become a chat grant, and
//! changing the workspace invalidates every grant and pending request.
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    pub chat: String,
    pub turn: String,
    pub workspace: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Once,
    Message,
    Chat,
    Reject,
}
#[derive(Default)]
pub struct Approvals {
    active: HashMap<String, Scope>,
    pending: HashMap<String, Scope>,
    messages: HashSet<Scope>,
    chats: HashSet<(String, String)>,
}
impl Approvals {
    pub fn start(&mut self, scope: Scope) -> Result<(), &'static str> {
        if self.active.contains_key(&scope.chat) {
            return Err("Stop the current response first");
        }
        self.chats
            .retain(|(chat, workspace)| chat != &scope.chat || workspace == &scope.workspace);
        self.active.insert(scope.chat.clone(), scope);
        Ok(())
    }
    pub fn allowed(&self, scope: &Scope) -> bool {
        self.active.get(&scope.chat) == Some(scope)
            && (self.messages.contains(scope)
                || self
                    .chats
                    .contains(&(scope.chat.clone(), scope.workspace.clone())))
    }
    pub fn request(&mut self, id: String, scope: &Scope) -> Result<(), &'static str> {
        if self.active.get(&scope.chat) != Some(scope) {
            return Err("This action has stopped");
        }
        if self.pending.contains_key(&id) {
            return Err("This approval already exists");
        }
        self.pending.insert(id, scope.clone());
        Ok(())
    }
    pub fn reply(&mut self, id: &str, decision: Decision) -> Result<bool, &'static str> {
        let scope = self
            .pending
            .remove(id)
            .ok_or("This approval is no longer pending")?;
        if self.active.get(&scope.chat) != Some(&scope) {
            return Err("This action has stopped");
        }
        match decision {
            Decision::Message => {
                self.messages.insert(scope);
            }
            Decision::Chat => {
                self.chats.insert((scope.chat, scope.workspace));
            }
            _ => {}
        }
        Ok(decision != Decision::Reject)
    }
    pub fn finish(&mut self, scope: &Scope) {
        if self.active.get(&scope.chat) == Some(scope) {
            self.active.remove(&scope.chat);
        }
        self.pending.retain(|_, value| value != scope);
        self.messages.remove(scope);
    }
    pub fn revoke(&mut self, chat: &str) {
        self.chats.retain(|(id, _)| id != chat);
        self.messages.retain(|s| s.chat != chat);
    }
    pub fn delete_history(&mut self, chat: Option<&str>) -> Result<(), &'static str> {
        if !self.active.is_empty() {
            return Err("Stop the current response before deleting history");
        }
        match chat {
            Some(chat) => {
                self.revoke(chat);
                self.pending.retain(|_, s| s.chat != chat);
            }
            None => {
                self.chats.clear();
                self.messages.clear();
                self.pending.clear();
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn scope(turn: &str) -> Scope {
        Scope {
            chat: "chat".into(),
            turn: turn.into(),
            workspace: "ssh:host/folder".into(),
        }
    }
    #[test]
    fn once_message_and_chat_have_different_lifetimes() {
        let mut grants = Approvals::default();
        let first = scope("1");
        grants.start(first.clone()).unwrap();
        grants.request("one".into(), &first).unwrap();
        assert!(grants.reply("one", Decision::Once).unwrap());
        assert!(!grants.allowed(&first));
        assert!(grants.reply("one", Decision::Chat).is_err());
        grants.request("two".into(), &first).unwrap();
        grants.reply("two", Decision::Message).unwrap();
        assert!(grants.allowed(&first));
        grants.finish(&first);
        let next = scope("2");
        grants.start(next.clone()).unwrap();
        assert!(!grants.allowed(&next));
        grants.request("three".into(), &next).unwrap();
        grants.reply("three", Decision::Chat).unwrap();
        grants.finish(&next);
        let third = scope("3");
        grants.start(third.clone()).unwrap();
        assert!(grants.allowed(&third));
        grants.revoke("chat");
        assert!(!grants.allowed(&third));
    }
    #[test]
    fn cancellation_and_workspace_changes_cannot_reuse_approvals() {
        let mut grants = Approvals::default();
        let first = scope("1");
        grants.start(first.clone()).unwrap();
        grants.request("one".into(), &first).unwrap();
        grants.finish(&first);
        assert!(grants.reply("one", Decision::Chat).is_err());
        let mut other = scope("2");
        other.workspace = "windows:C:/folder".into();
        grants.start(other.clone()).unwrap();
        assert!(!grants.allowed(&other));
        assert!(!grants.allowed(&first));
        assert!(grants.delete_history(None).is_err());
    }
}
