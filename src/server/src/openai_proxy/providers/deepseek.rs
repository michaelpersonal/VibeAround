use std::collections::BTreeSet;
use std::sync::LazyLock;

use common::profiles::schema::DeepSeekProviderSettings;
use dashmap::DashMap;
use serde_json::{json, Map, Value};

use crate::openai_proxy::reasoning_blob::decode_reasoning_content;

static REASONING_BY_CALL_ID: LazyLock<DashMap<String, String>> = LazyLock::new(DashMap::new);
const MISSING_REASONING_CONTENT_FALLBACK: &str =
    "Previous DeepSeek reasoning content is unavailable from the local proxy.";

#[derive(Debug, Clone)]
pub struct DeepSeekProxyAdapter {
    profile_id: String,
    settings: DeepSeekProviderSettings,
    stream_reasoning_content: String,
    stream_call_ids: BTreeSet<String>,
}

impl DeepSeekProxyAdapter {
    pub fn new(profile_id: String, settings: DeepSeekProviderSettings) -> Self {
        Self {
            profile_id,
            settings,
            stream_reasoning_content: String::new(),
            stream_call_ids: BTreeSet::new(),
        }
    }

    pub fn prepare_chat_request(&mut self, original_request: &Value, chat_request: &mut Value) {
        if self.settings.thinking && self.settings.replay_reasoning_content {
            persist_reasoning_from_responses_input(&self.profile_id, original_request);
            inject_reasoning_content(&self.profile_id, chat_request);
        }

        let Some(request) = chat_request.as_object_mut() else {
            return;
        };

        request.insert(
            "thinking".to_string(),
            json!({
                "type": if self.settings.thinking {
                    "enabled"
                } else {
                    "disabled"
                },
            }),
        );
    }

    pub fn observe_chat_completion(&mut self, completion: &Value) {
        if !self.settings.thinking || !self.settings.replay_reasoning_content {
            return;
        }
        let Some(message) = completion
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
        else {
            return;
        };
        let Some(reasoning_content) = message.get("reasoning_content").and_then(Value::as_str)
        else {
            return;
        };
        persist_reasoning_for_tool_calls(&self.profile_id, reasoning_content, message);
    }

    pub fn observe_chat_stream_chunk(&mut self, chunk: &Value) {
        if !self.settings.thinking || !self.settings.replay_reasoning_content {
            return;
        }

        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
            return;
        };
        for choice in choices {
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(reasoning_delta) = delta.get("reasoning_content").and_then(Value::as_str) {
                self.stream_reasoning_content.push_str(reasoning_delta);
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (fallback_index, tool_call) in tool_calls.iter().enumerate() {
                    self.observe_tool_call_delta(fallback_index, tool_call);
                }
            }
            if choice.get("finish_reason").and_then(Value::as_str) == Some("tool_calls") {
                self.persist_stream_reasoning();
            }
        }
    }

    fn observe_tool_call_delta(&mut self, _fallback_index: usize, tool_call: &Value) {
        if let Some(call_id) = tool_call.get("id").and_then(Value::as_str) {
            self.stream_call_ids.insert(call_id.to_string());
        }
    }

    fn persist_stream_reasoning(&mut self) {
        if self.stream_reasoning_content.is_empty() || self.stream_call_ids.is_empty() {
            return;
        }
        for call_id in &self.stream_call_ids {
            store_reasoning(&self.profile_id, call_id, &self.stream_reasoning_content);
        }
        self.stream_reasoning_content.clear();
        self.stream_call_ids.clear();
    }
}

fn inject_reasoning_content(profile_id: &str, chat_request: &mut Value) {
    let Some(messages) = chat_request
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    else {
        return;
    };

    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        if message.get("reasoning_content").is_some() {
            continue;
        }
        let reasoning_content = tool_calls
            .iter()
            .filter_map(|tool_call| tool_call.get("id").and_then(Value::as_str))
            .find_map(|call_id| lookup_reasoning(profile_id, call_id))
            .unwrap_or_else(|| MISSING_REASONING_CONTENT_FALLBACK.to_string());
        if let Some(obj) = message.as_object_mut() {
            obj.insert(
                "reasoning_content".to_string(),
                Value::String(reasoning_content),
            );
        }
    }
}

fn persist_reasoning_from_responses_input(profile_id: &str, responses_request: &Value) {
    let Some(items) = responses_request.get("input").and_then(Value::as_array) else {
        return;
    };

    let mut active_reasoning: Option<String> = None;
    let mut pending_call_ids: Vec<String> = Vec::new();

    for item in items {
        let Some(obj) = item.as_object() else {
            active_reasoning = None;
            pending_call_ids.clear();
            continue;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                if let Some(reasoning_content) = reasoning_content_from_item(obj) {
                    for call_id in pending_call_ids.drain(..) {
                        store_reasoning(profile_id, &call_id, &reasoning_content);
                    }
                    active_reasoning = Some(reasoning_content);
                }
            }
            Some("function_call") => {
                let Some(call_id) = obj
                    .get("call_id")
                    .or_else(|| obj.get("id"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                if let Some(reasoning_content) = &active_reasoning {
                    store_reasoning(profile_id, call_id, reasoning_content);
                } else {
                    pending_call_ids.push(call_id.to_string());
                }
            }
            Some("function_call_output") => {}
            _ => {
                active_reasoning = None;
                pending_call_ids.clear();
            }
        }
    }
}

fn reasoning_content_from_item(item: &Map<String, Value>) -> Option<String> {
    if let Some(reasoning_content) = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .and_then(decode_reasoning_content)
        .filter(|content| !content.is_empty())
    {
        return Some(reasoning_content);
    }

    let content = item.get("content").and_then(Value::as_array)?;
    let text = content
        .iter()
        .filter_map(|part| {
            let part = part.as_object()?;
            if part.get("type").and_then(Value::as_str) != Some("reasoning_text") {
                return None;
            }
            part.get("text").and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn persist_reasoning_for_tool_calls(profile_id: &str, reasoning_content: &str, message: &Value) {
    if reasoning_content.is_empty() {
        return;
    }
    let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return;
    };
    for tool_call in tool_calls {
        if let Some(call_id) = tool_call.get("id").and_then(Value::as_str) {
            store_reasoning(profile_id, call_id, reasoning_content);
        }
    }
}

fn store_reasoning(profile_id: &str, call_id: &str, reasoning_content: &str) {
    REASONING_BY_CALL_ID.insert(
        reasoning_key(profile_id, call_id),
        reasoning_content.to_string(),
    );
}

fn lookup_reasoning(profile_id: &str, call_id: &str) -> Option<String> {
    REASONING_BY_CALL_ID
        .get(&reasoning_key(profile_id, call_id))
        .map(|entry| entry.value().clone())
}

fn reasoning_key(profile_id: &str, call_id: &str) -> String {
    format!("{profile_id}:{call_id}")
}

#[cfg(test)]
mod tests {
    use common::profiles::schema::DeepSeekProviderSettings;
    use serde_json::json;

    use crate::openai_proxy::reasoning_blob::encode_reasoning_content;

    use super::DeepSeekProxyAdapter;

    #[test]
    fn default_settings_disable_thinking_for_existing_profiles() {
        let mut adapter = DeepSeekProxyAdapter::new(
            "deepseek-profile".to_string(),
            DeepSeekProviderSettings::default(),
        );
        let mut request = json!({
            "model": "deepseek-v4-flash",
            "messages": [{ "role": "user", "content": "hello" }],
        });

        adapter.prepare_chat_request(&json!({ "input": "hello" }), &mut request);

        assert_eq!(request["thinking"]["type"], "disabled");
    }

    #[test]
    fn replays_reasoning_content_for_matching_tool_call() {
        let settings = DeepSeekProviderSettings {
            thinking: true,
            replay_reasoning_content: true,
        };
        let mut adapter =
            DeepSeekProxyAdapter::new("deepseek-replay".to_string(), settings.clone());
        adapter.observe_chat_stream_chunk(&json!({
            "choices": [{
                "delta": { "reasoning_content": "I should inspect cwd." },
                "finish_reason": null
            }]
        }));
        adapter.observe_chat_stream_chunk(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_pwd",
                        "type": "function",
                        "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }));

        let mut next_request = json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_pwd",
                    "type": "function",
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            }, {
                "role": "tool",
                "tool_call_id": "call_pwd",
                "content": "/Users/jazzen/Development"
            }]
        });
        let mut adapter = DeepSeekProxyAdapter::new("deepseek-replay".to_string(), settings);

        adapter.prepare_chat_request(&json!({ "input": [] }), &mut next_request);

        assert_eq!(next_request["thinking"]["type"], "enabled");
        assert_eq!(
            next_request["messages"][0]["reasoning_content"],
            "I should inspect cwd."
        );
    }

    #[test]
    fn replays_reasoning_content_from_responses_history() {
        let settings = DeepSeekProviderSettings {
            thinking: true,
            replay_reasoning_content: true,
        };
        let original_request = json!({
            "input": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [],
                    "encrypted_content": encode_reasoning_content("Call pwd, then answer.")
                },
                {
                    "type": "function_call",
                    "call_id": "call_pwd",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_pwd",
                    "output": "/Users/jazzen/Development"
                }
            ]
        });
        let mut chat_request = json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_pwd",
                    "type": "function",
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            }, {
                "role": "tool",
                "tool_call_id": "call_pwd",
                "content": "/Users/jazzen/Development"
            }]
        });
        let mut adapter = DeepSeekProxyAdapter::new("deepseek-history".to_string(), settings);

        adapter.prepare_chat_request(&original_request, &mut chat_request);

        assert_eq!(
            chat_request["messages"][0]["reasoning_content"],
            "Call pwd, then answer."
        );
    }

    #[test]
    fn adds_fallback_reasoning_content_for_existing_history() {
        let settings = DeepSeekProviderSettings {
            thinking: true,
            replay_reasoning_content: true,
        };
        let mut chat_request = json!({
            "model": "deepseek-v4-flash",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_old",
                    "type": "function",
                    "function": { "name": "exec_command", "arguments": "{\"cmd\":\"pwd\"}" }
                }]
            }]
        });
        let mut adapter = DeepSeekProxyAdapter::new("deepseek-fallback".to_string(), settings);

        adapter.prepare_chat_request(&json!({ "input": [] }), &mut chat_request);

        assert_eq!(
            chat_request["messages"][0]["reasoning_content"],
            super::MISSING_REASONING_CONTENT_FALLBACK
        );
    }
}
