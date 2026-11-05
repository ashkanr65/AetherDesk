//! Keep tool evidence readable without filling the conversation with raw JSON.
use eframe::egui;
use serde_json::Value;

#[derive(Default)]
pub(super) struct ToolProgress {
    failures: std::collections::BTreeMap<String, usize>,
    failed_rounds: usize,
}
impl ToolProgress {
    pub(super) fn signature(name: &str, args: &Value) -> String {
        format!("{name}:{args}")
    }
    pub(super) fn can_retry(&self, signature: &str) -> bool {
        self.failures.get(signature).copied().unwrap_or(0) < 2
    }
    pub(super) fn record(&mut self, signature: String, value: &Value) -> bool {
        let failed = failed(value);
        if failed {
            *self.failures.entry(signature).or_default() += 1;
        }
        failed
    }
    pub(super) fn finish_round(&mut self, failed: bool) {
        self.failed_rounds = if failed { self.failed_rounds + 1 } else { 0 };
    }
    pub(super) fn needs_summary(&self) -> bool {
        self.failed_rounds >= 3
    }
}

pub(super) fn failed(value: &Value) -> bool {
    value.get("error").is_some()
        || value
            .get("exit_status")
            .is_some_and(|status| status.as_i64() != Some(0))
}
/// Compact only the model's copy of tool evidence. Full details stay in chat.
pub(super) fn request_context(messages: &[Value], limit: usize) -> Result<Vec<Value>, String> {
    if let Ok(context) = crate::ai_models::model_context(messages, limit) {
        return Ok(context);
    }
    let count = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .count();
    if count == 0 {
        return crate::ai_models::model_context(messages, limit);
    }
    let budget = (limit / count / 4).clamp(64, 4096);
    let mut compact = messages.to_vec();
    for message in &mut compact {
        if message["role"] != "tool" {
            continue;
        }
        let Some(content) = message["content"].as_str() else {
            continue;
        };
        if content.len() <= budget {
            continue;
        }
        let mut end = budget / 2;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        let value = serde_json::from_str::<Value>(content).unwrap_or(Value::Null);
        let mut shortened = serde_json::json!({"truncated":true,"notice":"Tool evidence shortened for context. Full details remain in chat; request a narrower result if needed.","excerpt":&content[..end]});
        if let Some(status) = value.get("exit_status") {
            shortened["exit_status"] = status.clone();
        }
        if value.get("error").is_some() {
            shortened["error"] = Value::String(
                "Tool failed; see the excerpt/details. Correct the request before retrying.".into(),
            );
        }
        message["content"] = Value::String(shortened.to_string());
    }
    crate::ai_models::model_context(&compact, limit)
}

fn tool_detail(content: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return content.to_owned();
    };
    if let Some(error) = value["error"].as_str() {
        return error.to_owned();
    }
    if value.get("stdout").is_some() || value.get("stderr").is_some() {
        let mut parts = Vec::new();
        if let Some(status) = value.get("exit_status") {
            parts.push(format!("Exit status: {status}"));
        }
        for key in ["stdout", "stderr"] {
            if let Some(text) = value[key].as_str().filter(|s| !s.is_empty()) {
                parts.push(text.to_owned());
            }
        }
        return parts.join("\n");
    }
    if let Some(entries) = value.as_array() {
        return entries
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| entry.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| content.into())
}

pub(super) fn message_ui(
    ui: &mut egui::Ui,
    message: &Value,
    selection: &mut super::text_selection::Frame,
) {
    let role = message["role"].as_str().unwrap_or("");
    if role == "system" {
        return;
    }
    if role == "tool" {
        let content = message["content"].as_str().unwrap_or("");
        let failure = serde_json::from_str::<Value>(content).is_ok_and(|v| failed(&v));
        egui::CollapsingHeader::new(if failure {
            "Tool failed · details"
        } else {
            "Tool result · details"
        })
        .id_salt(message["tool_call_id"].as_str().unwrap_or(content))
        .show(ui, |ui| {
            selection.text(ui, &tool_detail(content));
        });
        return;
    }
    let text = if let Some(text) = message["content"].as_str() {
        text.to_owned()
    } else {
        message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| {
                if part["type"] == "image_url" {
                    Some("[Attached screenshot]")
                } else {
                    part["text"].as_str()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    if text.trim().is_empty() {
        return;
    }
    ui.add(egui::Label::new(egui::RichText::new(role).strong()).selectable(false));
    selection.text(ui, &text);
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tool_results_decode_command_output_and_classify_nonzero_exits() {
        let result = json!({"stdout":"first\nsecond", "stderr":"problem", "exit_status":1});
        assert!(failed(&result));
        assert!(!failed(&json!({"exit_status":0,"stdout":""})));
        assert!(failed(&json!({"error":"limit"})));
        assert!(tool_detail(&result.to_string()).contains("first\nsecond\nproblem"));
        assert_eq!(tool_detail("[\"one\",\"two\"]"), "one\ntwo");
    }
    #[test]
    fn repeated_tool_failures_stop_reexecution_and_require_a_final_answer() {
        let mut progress = ToolProgress::default();
        let signature = ToolProgress::signature("run_command", &json!({"command":"broken"}));
        for _ in 0..2 {
            assert!(progress.can_retry(&signature));
            assert!(progress.record(signature.clone(), &json!({"exit_status":1})));
            progress.finish_round(true);
        }
        assert!(!progress.can_retry(&signature));
        assert!(progress.can_retry(&ToolProgress::signature(
            "run_command",
            &json!({"command":"corrected"})
        )));
        progress.finish_round(true);
        assert!(progress.needs_summary());
        progress.finish_round(false);
        assert!(!progress.needs_summary());
    }
    #[test]
    fn large_tool_results_leave_room_for_an_answer_without_altering_saved_evidence() {
        let messages = vec![
            serde_json::json!({"role":"user","content":"Check licences"}),
            serde_json::json!({"role":"assistant","tool_calls":[{"id":"one","type":"function","function":{"name":"run_command","arguments":"{}"}}]}),
            serde_json::json!({"role":"tool","tool_call_id":"one","content":serde_json::json!({"exit_status":1,"stderr":"error ñ".repeat(10000)}).to_string()}),
        ];
        let before = messages.clone();
        let context = request_context(&messages, 8000).unwrap();
        assert_eq!(messages, before);
        let result: Value = serde_json::from_str(context[2]["content"].as_str().unwrap()).unwrap();
        assert_eq!(result["exit_status"], 1);
        assert_eq!(result["truncated"], true);
        assert_eq!(
            context[1]["tool_calls"][0]["id"],
            context[2]["tool_call_id"]
        );
        let oversized = vec![serde_json::json!({"role":"user","content":"x".repeat(20000)})];
        assert!(
            request_context(&oversized, 8000).is_err(),
            "never silently shorten the user's prompt"
        );
    }
}
