//! Provider-declared capabilities. Missing metadata remains unknown; the UI
//! keeps every listed model available for explicit user selection.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelCost {
    Free,
    Paid,
    #[default]
    Unknown,
}
fn model_cost(value: &Value) -> ModelCost {
    let Some(pricing) = value["pricing"].as_object() else {
        return ModelCost::Unknown;
    };
    let amount = |v: &Value| {
        v.as_f64()
            .or_else(|| v.as_str()?.parse::<f64>().ok())
            .filter(|n| n.is_finite() && *n >= 0.0)
    };
    if pricing.values().any(|v| amount(v).is_some_and(|n| n > 0.0)) {
        return ModelCost::Paid;
    }
    if pricing.get("prompt").and_then(amount) == Some(0.0)
        && pricing.get("completion").and_then(amount) == Some(0.0)
        && pricing.values().all(|v| amount(v) == Some(0.0))
    {
        ModelCost::Free
    } else {
        ModelCost::Unknown
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub context_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub parameters: BTreeSet<String>,
    pub tools: Option<bool>,
    pub images: Option<bool>,
    pub reasoning: Option<Reasoning>,
    pub cost: ModelCost,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reasoning {
    pub efforts: Vec<String>,
    pub mandatory: bool,
    pub supports_budget: bool,
}
impl Model {
    pub fn from_provider(value: &Value) -> Option<Self> {
        let id = value["id"].as_str()?.trim();
        if id.is_empty() || id.len() > 300 || id.chars().any(char::is_control) {
            return None;
        }
        let parameters = value["supported_parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|s| {
                s.len() <= 80
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
            .map(str::to_owned)
            .collect();
        let image_modalities = value["architecture"]["input_modalities"].as_array();
        let tools = value["supported_parameters"]
            .as_array()
            .map(|values| values.iter().any(|v| v.as_str() == Some("tools")));
        let images = image_modalities.map(|values| {
            values
                .iter()
                .any(|v| v.as_str().is_some_and(|s| matches!(s, "image" | "video")))
        });
        let reasoning = value["reasoning"].as_object().map(|meta| Reasoning {
            efforts: if meta.get("supported_efforts").is_some_and(Value::is_null) {
                ["max", "xhigh", "high", "medium", "low", "minimal", "none"]
                    .into_iter()
                    .filter(|effort| {
                        *effort != "none"
                            || meta.get("mandatory").and_then(Value::as_bool) != Some(true)
                    })
                    .map(str::to_owned)
                    .collect()
            } else {
                meta.get("supported_efforts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|s| {
                        matches!(
                            *s,
                            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
                        )
                    })
                    .filter(|effort| {
                        *effort != "none"
                            || meta.get("mandatory").and_then(Value::as_bool) != Some(true)
                    })
                    .map(str::to_owned)
                    .collect()
            },
            mandatory: meta
                .get("mandatory")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            supports_budget: meta
                .get("supports_max_tokens")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        let output_tokens = value["top_provider"]["max_completion_tokens"]
            .as_u64()
            .or_else(|| value["max_completion_tokens"].as_u64())
            .or_else(|| value["max_output_tokens"].as_u64())
            .filter(|n| (1..=1_000_000).contains(n));
        let context_tokens = value["context_length"]
            .as_u64()
            .or_else(|| value["max_model_len"].as_u64())
            .filter(|n| (256..=1_000_000).contains(n));
        Some(Self {
            id: id.into(),
            name: value["name"]
                .as_str()
                .filter(|s| s.len() <= 300)
                .unwrap_or(id)
                .into(),
            context_tokens,
            output_tokens,
            parameters,
            tools,
            images,
            reasoning,
            cost: model_cost(value),
        })
    }
    pub fn reasoning_request(
        &self,
        effort: Option<&str>,
        budget: u32,
        output: u32,
    ) -> Option<Value> {
        let options = self.reasoning.as_ref()?;
        let effort = effort.filter(|e| {
            options.efforts.iter().any(|v| v == *e) && !(options.mandatory && *e == "none")
        });
        // Explicit disable wins over an old saved budget; a reasoning budget
        // must leave room for visible output and never exceed the reply limit.
        if effort == Some("none") {
            return Some(serde_json::json!({"effort":"none"}));
        }
        let budget = budget.min(output.saturating_sub(1));
        if options.supports_budget && budget > 0 {
            Some(serde_json::json!({"max_tokens":budget}))
        } else {
            effort.map(|effort| serde_json::json!({"effort":effort}))
        }
    }
    pub fn manual(id: &str) -> Self {
        Self {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }
}
fn turn_context_bytes(turn: &[Value]) -> Result<usize, String> {
    let mut bytes = serde_json::to_vec(turn).map_err(|e| e.to_string())?.len();
    for message in turn {
        for part in message["content"].as_array().into_iter().flatten() {
            if part["type"] == "image_url" {
                if let Some(url) = part["image_url"]["url"].as_str() {
                    // Vision inputs are not tokenized as their base64 text.
                    // Reserve 4096 estimated image tokens in the existing
                    // three-bytes-per-token heuristic, independently of PNG size.
                    // Providers remain authoritative about actual vision usage.
                    let encoded = serde_json::to_vec(url).map_err(|e| e.to_string())?.len();
                    bytes = bytes.saturating_sub(encoded).saturating_add(4096 * 3);
                }
            }
        }
    }
    Ok(bytes)
}
pub fn model_context(messages: &[Value], limit: usize) -> Result<Vec<Value>, String> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let mut groups: Vec<Vec<Value>> = Vec::new();
    let mut current = Vec::new();
    for message in messages.iter().filter(|m| m["role"] != "system") {
        if message["role"] == "user" && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.push(message.clone());
    }
    if !current.is_empty() {
        groups.push(current);
    }
    let mut selected = Vec::new();
    let mut used = 0usize;
    for group in groups.into_iter().rev() {
        let bytes = turn_context_bytes(&group)?;
        if selected.is_empty() && bytes > limit {
            return Err("The latest chat turn exceeds this model's context budget. Shorten the message or raise the context setting.".into());
        }
        if used.saturating_add(bytes) > limit {
            break;
        }
        used += bytes;
        selected.push(group);
    }
    selected.reverse();
    Ok(selected.into_iter().flatten().collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn screenshot_payload_is_not_text_context_and_keeps_complete_turns() {
        let image = |bytes: usize| json!({"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}", "A".repeat(bytes))}});
        let turn = |bytes| {
            vec![
                json!({"role":"user","content":[{"type":"text","text":"Describe these pixels"},image(bytes)]}),
                json!({"role":"assistant","tool_calls":[{"id":"read","type":"function","function":{"name":"read_file","arguments":"{}"}}]}),
                json!({"role":"tool","tool_call_id":"read","content":"result"}),
            ]
        };
        let small = turn(1000);
        let large = turn(1024 * 1024);
        assert_eq!(
            turn_context_bytes(&small).unwrap(),
            turn_context_bytes(&large).unwrap()
        );
        let budget = turn_context_bytes(&large).unwrap();
        assert!(serde_json::to_vec(&large).unwrap().len() > budget);
        assert_eq!(model_context(&large, budget).unwrap(), large);
        assert!(model_context(&large, budget - 1).is_err());
        let mut history = vec![
            json!({"role":"user","content":"older"}),
            json!({"role":"assistant","content":"old reply"}),
        ];
        history.extend(large.clone());
        assert_eq!(model_context(&history, budget).unwrap(), large);
        let two = vec![json!({"role":"user","content":[image(1000),image(1000)]})];
        assert!(
            model_context(&two, budget).is_err(),
            "images must still reserve context"
        );
    }
    #[test]
    fn pricing_filters_require_declared_prices_and_preserve_unknown_models() {
        for (pricing, expected) in [
            (json!({"prompt":"0","completion":"0"}), ModelCost::Free),
            (json!({"prompt":0,"completion":"0.0001"}), ModelCost::Paid),
            (
                json!({"prompt":"0","completion":"0","request":"0.01"}),
                ModelCost::Paid,
            ),
            (json!({"prompt":"0"}), ModelCost::Unknown),
            (json!({"prompt":"-1","completion":"0"}), ModelCost::Unknown),
            (json!({"prompt":"NaN","completion":"0"}), ModelCost::Unknown),
            (Value::Null, ModelCost::Unknown),
        ] {
            let model = Model::from_provider(
                &json!({"id":"custom/free-name-is-not-proof","pricing":pricing}),
            )
            .unwrap();
            assert_eq!(model.cost, expected);
            let saved = serde_json::to_value(&model).unwrap();
            assert_eq!(
                serde_json::from_value::<Model>(saved).unwrap().cost,
                expected
            );
        }
        let old: Model = serde_json::from_value(json!({"id":"old","name":"old"})).unwrap();
        assert_eq!(old.cost, ModelCost::Unknown);
    }
    #[test]
    fn reasoning_requests_respect_mandatory_models_and_visible_reply_budget() {
        let model = Model::from_provider(&json!({"id":"model","reasoning":{"supported_efforts":null,"supports_max_tokens":true}})).unwrap();
        assert_eq!(model.reasoning_request(None, 0, 2048), None);
        assert_eq!(
            model.reasoning_request(Some("high"), 0, 2048),
            Some(json!({"effort":"high"}))
        );
        assert_eq!(
            model.reasoning_request(Some("high"), 5000, 2048),
            Some(json!({"max_tokens":2047}))
        );
        assert_eq!(
            model.reasoning_request(Some("none"), 5000, 2048),
            Some(json!({"effort":"none"}))
        );
        let mandatory = Model::from_provider(
            &json!({"id":"required","reasoning":{"supported_efforts":null,"mandatory":true}}),
        )
        .unwrap();
        assert_eq!(mandatory.reasoning_request(Some("none"), 0, 2048), None);
        assert_eq!(
            Model::manual("unknown").reasoning_request(Some("high"), 100, 2048),
            None
        );
    }
    #[test]
    fn reasoning_metadata_distinguishes_unrestricted_missing_and_mandatory_efforts() {
        let all = Model::from_provider(&json!({"id":"all","reasoning":{"supported_efforts":null}}))
            .unwrap()
            .reasoning
            .unwrap();
        assert!(all.efforts.contains(&"high".into()));
        assert!(all.efforts.contains(&"none".into()));
        let automatic = Model::from_provider(&json!({"id":"auto","reasoning":{}}))
            .unwrap()
            .reasoning
            .unwrap();
        assert!(automatic.efforts.is_empty());
        let required = Model::from_provider(&json!({"id":"required","reasoning":{"supported_efforts":["none","high"],"mandatory":true}})).unwrap().reasoning.unwrap();
        assert_eq!(required.efforts, ["high"]);
        let required_all = Model::from_provider(
            &json!({"id":"required-all","reasoning":{"supported_efforts":null,"mandatory":true}}),
        )
        .unwrap()
        .reasoning
        .unwrap();
        assert!(!required_all.efforts.contains(&"none".into()));
        assert!(Model::manual("unknown").reasoning.is_none());
    }
    #[test]
    fn capabilities_preserve_paid_models_and_unknown_manual_model_metadata() {
        let model=Model::from_provider(&json!({"id":"vendor/paid","pricing":{"prompt":"2"},"context_length":32000,"top_provider":{"max_completion_tokens":2048},"supported_parameters":["tools","reasoning"],"architecture":{"input_modalities":["text","image"]},"reasoning":{"supported_efforts":["low","unsupported"],"mandatory":true}})).unwrap();
        assert_eq!(model.id, "vendor/paid");
        assert_eq!(model.tools, Some(true));
        assert_eq!(model.images, Some(true));
        assert_eq!(model.output_tokens, Some(2048));
        assert_eq!(model.reasoning.unwrap().efforts, vec!["low"]);
        let manual = Model::manual("other/model");
        assert_eq!(manual.tools, None);
        assert_eq!(manual.images, None);
        assert!(Model::from_provider(&json!({"id":"in\nvalid"})).is_none());
    }
    #[test]
    fn context_trimming_keeps_complete_tool_turns_and_rejects_an_oversized_latest_turn() {
        let last = vec![
            json!({"role":"user","content":"latest"}),
            json!({"role":"assistant","tool_calls":[{"id":"call"}]}),
            json!({"role":"tool","tool_call_id":"call","content":"result"}),
        ];
        let mut all = vec![
            json!({"role":"system","content":"old system"}),
            json!({"role":"user","content":"old"}),
            json!({"role":"assistant","content":"old answer"}),
        ];
        all.extend(last.clone());
        let budget = serde_json::to_vec(&last).unwrap().len();
        assert_eq!(model_context(&all, budget).unwrap(), last);
        assert!(model_context(&all, budget - 1).is_err());
    }
}
