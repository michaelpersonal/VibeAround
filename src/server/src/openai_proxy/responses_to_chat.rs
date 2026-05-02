use serde_json::{json, Map, Value};

use super::error::{ProxyTransformError, Result};

pub fn responses_to_chat_request(body: Value) -> Result<Value> {
    let root = body
        .as_object()
        .ok_or(ProxyTransformError::ExpectedObject("Responses request"))?;

    let model = root
        .get("model")
        .cloned()
        .ok_or(ProxyTransformError::MissingField("model"))?;
    let input = root
        .get("input")
        .ok_or(ProxyTransformError::MissingField("input"))?;
    ensure_stateless_request(root)?;

    let mut messages = Vec::new();
    if let Some(instructions) = root.get("instructions").and_then(value_to_text) {
        if !instructions.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": instructions,
            }));
        }
    }
    messages.extend(input_to_messages(input)?);
    let messages = normalize_chat_messages(messages);

    let mut out = Map::new();
    out.insert("model".to_string(), model);
    out.insert("messages".to_string(), Value::Array(messages));

    copy_fields(
        root,
        &mut out,
        &[
            "frequency_penalty",
            "metadata",
            "parallel_tool_calls",
            "presence_penalty",
            "prompt_cache_key",
            "safety_identifier",
            "seed",
            "service_tier",
            "store",
            "stream",
            "temperature",
            "top_logprobs",
            "top_p",
            "user",
        ],
    );

    if let Some(max_output_tokens) = root.get("max_output_tokens") {
        out.insert("max_tokens".to_string(), max_output_tokens.clone());
    }

    if let Some(tools) = root.get("tools").and_then(Value::as_array) {
        let tools = convert_tools(tools)?;
        if !tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(tools));
        }
    }

    if let Some(tool_choice) = root.get("tool_choice") {
        out.insert("tool_choice".to_string(), convert_tool_choice(tool_choice)?);
    }

    if let Some(response_format) = convert_text_format(root.get("text"))? {
        out.insert("response_format".to_string(), response_format);
    }

    Ok(Value::Object(out))
}

fn ensure_stateless_request(root: &Map<String, Value>) -> Result<()> {
    if root
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ProxyTransformError::Unsupported(
            "`previous_response_id` requires local response-state expansion before Chat conversion"
                .to_string(),
        ));
    }

    if root
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        return Err(ProxyTransformError::Unsupported(
            "`conversation` requires local conversation-state expansion before Chat conversion"
                .to_string(),
        ));
    }

    Ok(())
}

fn input_to_messages(input: &Value) -> Result<Vec<Value>> {
    match input {
        Value::String(text) => Ok(vec![json!({
            "role": "user",
            "content": text,
        })]),
        Value::Array(items) => {
            let mut messages = Vec::new();
            for item in items {
                match item {
                    Value::String(text) => messages.push(json!({
                        "role": "user",
                        "content": text,
                    })),
                    Value::Object(obj) => convert_input_item(obj, &mut messages)?,
                    _ => {
                        return Err(ProxyTransformError::Unsupported(
                            "Responses input array contains a non-object item".to_string(),
                        ));
                    }
                }
            }
            Ok(messages)
        }
        _ => Err(ProxyTransformError::Unsupported(
            "`input` must be a string or array for Chat conversion".to_string(),
        )),
    }
}

fn normalize_chat_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut system_parts = Vec::new();
    let mut rest = Vec::new();

    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("system") {
            if let Some(content) = message.get("content") {
                let text = value_to_string(content);
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
        } else {
            rest.push(message);
        }
    }

    if system_parts.is_empty() {
        return rest;
    }

    let mut normalized = vec![json!({
        "role": "system",
        "content": system_parts.join("\n\n"),
    })];
    normalized.extend(rest);
    normalized
}

fn convert_input_item(item: &Map<String, Value>, messages: &mut Vec<Value>) -> Result<()> {
    match item.get("type").and_then(Value::as_str) {
        None | Some("message") => {
            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
            let role = chat_compatible_role(role)?;
            let content = item
                .get("content")
                .map(content_to_text)
                .transpose()?
                .unwrap_or_default();
            messages.push(json!({
                "role": role,
                "content": content,
            }));
            Ok(())
        }
        Some("function_call") => {
            let call_id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("call_unknown");
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ProxyTransformError::MissingField("input[].name"))?;
            let arguments = item
                .get("arguments")
                .map(value_to_string)
                .unwrap_or_else(|| "{}".to_string());
            messages.push(json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    },
                }],
            }));
            Ok(())
        }
        Some("function_call_output") => {
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or(ProxyTransformError::MissingField("input[].call_id"))?;
            let output = item.get("output").map(value_to_string).unwrap_or_default();
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": output,
            }));
            Ok(())
        }
        Some("reasoning") => Ok(()),
        Some(other) => Err(ProxyTransformError::Unsupported(format!(
            "Responses input item type `{other}` cannot be represented in Chat Completions"
        ))),
    }
}

fn chat_compatible_role(role: &str) -> Result<&str> {
    match role {
        // OpenAI's newer Chat/Responses shape allows developer messages, but
        // many OpenAI-compatible chat providers still only accept the classic
        // system/user/assistant/tool roles. Preserve the instruction priority
        // by downgrading developer to system for chat-only upstreams.
        "developer" => Ok("system"),
        "system" | "user" | "assistant" | "tool" => Ok(role),
        other => Err(ProxyTransformError::Unsupported(format!(
            "message role `{other}` cannot be represented in Chat Completions"
        ))),
    }
}

fn content_to_text(content: &Value) -> Result<String> {
    match content {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => {
            let mut text = String::new();
            for part in parts {
                let Some(obj) = part.as_object() else {
                    return Err(ProxyTransformError::Unsupported(
                        "content part must be an object".to_string(),
                    ));
                };
                match obj.get("type").and_then(Value::as_str) {
                    Some("input_text") | Some("output_text") | Some("text") => {
                        if let Some(part_text) = obj.get("text").and_then(Value::as_str) {
                            text.push_str(part_text);
                        }
                    }
                    Some(other) => {
                        return Err(ProxyTransformError::Unsupported(format!(
                            "content part type `{other}` cannot be represented in a text-only Chat request"
                        )));
                    }
                    None => {
                        if let Some(part_text) = obj.get("text").and_then(Value::as_str) {
                            text.push_str(part_text);
                        }
                    }
                }
            }
            Ok(text)
        }
        Value::Null => Ok(String::new()),
        other => Ok(value_to_string(other)),
    }
}

fn convert_tools(tools: &[Value]) -> Result<Vec<Value>> {
    let mut converted = Vec::new();
    for tool in tools {
        let obj = tool
            .as_object()
            .ok_or(ProxyTransformError::ExpectedObject("tool"))?;
        match obj.get("type").and_then(Value::as_str) {
            Some("function") => {
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(ProxyTransformError::MissingField("tools[].name"))?;
                let mut function = Map::new();
                function.insert("name".to_string(), json!(name));
                copy_fields(obj, &mut function, &["description", "parameters", "strict"]);
                converted.push(json!({
                    "type": "function",
                    "function": Value::Object(function),
                }));
            }
            Some(other) => {
                tracing::debug!(
                    tool_type = other,
                    "dropping Responses built-in tool for Chat-only upstream"
                );
            }
            None => {
                return Err(ProxyTransformError::MissingField("tools[].type"));
            }
        }
    }
    Ok(converted)
}

fn convert_tool_choice(tool_choice: &Value) -> Result<Value> {
    match tool_choice {
        Value::String(_) => Ok(tool_choice.clone()),
        Value::Object(obj) => match obj.get("type").and_then(Value::as_str) {
            Some("function") => {
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(ProxyTransformError::MissingField("tool_choice.name"))?;
                Ok(json!({
                    "type": "function",
                    "function": { "name": name },
                }))
            }
            Some(other) => {
                tracing::debug!(
                    tool_choice_type = other,
                    "downgrading Responses built-in tool_choice to auto for Chat-only upstream"
                );
                Ok(json!("auto"))
            }
            None => Ok(tool_choice.clone()),
        },
        _ => Ok(tool_choice.clone()),
    }
}

fn convert_text_format(text: Option<&Value>) -> Result<Option<Value>> {
    let Some(format) = text.and_then(|v| v.get("format")) else {
        return Ok(None);
    };
    let Some(obj) = format.as_object() else {
        return Ok(None);
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("text") | None => Ok(None),
        Some("json_object") => Ok(Some(json!({ "type": "json_object" }))),
        Some("json_schema") => {
            let mut json_schema = Map::new();
            copy_fields(
                obj,
                &mut json_schema,
                &["name", "description", "schema", "strict"],
            );
            Ok(Some(json!({
                "type": "json_schema",
                "json_schema": Value::Object(json_schema),
            })))
        }
        Some(other) => Err(ProxyTransformError::Unsupported(format!(
            "Responses text format `{other}` cannot be represented in Chat Completions"
        ))),
    }
}

fn copy_fields(from: &Map<String, Value>, to: &mut Map<String, Value>, names: &[&str]) {
    for name in names {
        if let Some(value) = from.get(*name) {
            to.insert((*name).to_string(), value.clone());
        }
    }
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n"),
        ),
        _ => None,
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::responses_to_chat_request;

    #[test]
    fn converts_basic_responses_request_to_chat() {
        let chat = responses_to_chat_request(json!({
            "model": "deepseek-chat",
            "instructions": "Be brief.",
            "input": "Hello",
            "max_output_tokens": 256,
            "temperature": 0.2,
            "stream": true,
        }))
        .unwrap();

        assert_eq!(chat["model"], "deepseek-chat");
        assert_eq!(chat["max_tokens"], 256);
        assert_eq!(chat["temperature"], 0.2);
        assert_eq!(chat["stream"], true);
        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][0]["content"], "Be brief.");
        assert_eq!(chat["messages"][1]["role"], "user");
        assert_eq!(chat["messages"][1]["content"], "Hello");
    }

    #[test]
    fn converts_function_tool_shape() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "input": [{ "role": "user", "content": [{ "type": "input_text", "text": "weather?" }] }],
            "tools": [{
                "type": "function",
                "name": "get_weather",
                "description": "Read weather",
                "parameters": { "type": "object" },
                "strict": true
            }],
            "tool_choice": { "type": "function", "name": "get_weather" }
        }))
        .unwrap();

        assert_eq!(chat["tools"][0]["type"], "function");
        assert_eq!(chat["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(chat["tools"][0]["function"]["strict"], true);
        assert_eq!(chat["tool_choice"]["function"]["name"], "get_weather");
    }

    #[test]
    fn drops_responses_builtin_tools_for_chat_upstreams() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "input": "hello",
            "tools": [
                { "type": "web_search" },
                { "type": "function", "name": "list_files" }
            ],
            "tool_choice": { "type": "web_search" }
        }))
        .unwrap();

        assert_eq!(chat["tools"].as_array().unwrap().len(), 1);
        assert_eq!(chat["tools"][0]["function"]["name"], "list_files");
        assert_eq!(chat["tool_choice"], "auto");
    }

    #[test]
    fn drops_responses_reasoning_for_chat_compatibility() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "input": "hello",
            "reasoning": { "effort": "high" }
        }))
        .unwrap();

        assert!(chat.get("reasoning_effort").is_none());
        assert!(chat.get("reasoning").is_none());
    }

    #[test]
    fn converts_function_call_history() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "input": [
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "list_files",
                    "arguments": "{\"path\":\".\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": "[\"Cargo.toml\"]"
                }
            ]
        }))
        .unwrap();

        assert_eq!(chat["messages"][0]["role"], "assistant");
        assert_eq!(chat["messages"][0]["tool_calls"][0]["id"], "call_123");
        assert_eq!(chat["messages"][1]["role"], "tool");
        assert_eq!(chat["messages"][1]["tool_call_id"], "call_123");
    }

    #[test]
    fn downgrades_developer_role_for_chat_compatibility() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "input": [
                { "role": "developer", "content": "Answer tersely." },
                { "role": "user", "content": "Hello" }
            ]
        }))
        .unwrap();

        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(chat["messages"][0]["content"], "Answer tersely.");
        assert_eq!(chat["messages"][1]["role"], "user");
    }

    #[test]
    fn folds_all_instruction_roles_into_one_leading_system_message() {
        let chat = responses_to_chat_request(json!({
            "model": "chat-model",
            "instructions": "Global instructions.",
            "input": [
                { "role": "user", "content": "Hi" },
                { "role": "developer", "content": "Prefer JSON." },
                { "role": "system", "content": "Never reveal secrets." }
            ]
        }))
        .unwrap();

        assert_eq!(chat["messages"][0]["role"], "system");
        assert_eq!(
            chat["messages"][0]["content"],
            "Global instructions.\n\nPrefer JSON.\n\nNever reveal secrets."
        );
        assert_eq!(chat["messages"][1]["role"], "user");
    }

    #[test]
    fn rejects_stateful_responses_requests_until_proxy_expands_state() {
        let error = responses_to_chat_request(json!({
            "model": "chat-model",
            "previous_response_id": "resp_123",
            "input": "Continue"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("previous_response_id"));
    }
}
