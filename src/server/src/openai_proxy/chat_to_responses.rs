use std::collections::BTreeMap;

use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use super::error::{ProxyTransformError, Result};
use super::reasoning_blob::encode_reasoning_content;
use super::sse::ResponseStreamEvent;

pub fn chat_completion_to_response(chat: Value, original_request: &Value) -> Result<Value> {
    let root = chat
        .as_object()
        .ok_or(ProxyTransformError::ExpectedObject("Chat completion"))?;
    let choice = root
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(Value::as_object)
        .ok_or(ProxyTransformError::MissingField("choices[0]"))?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or(ProxyTransformError::MissingField("choices[0].message"))?;

    let response_id = response_id();
    let mut output = Vec::new();

    if let Some(reasoning_content) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        output.push(reasoning_item(
            reasoning_id(),
            Some(encode_reasoning_content(reasoning_content)),
            "completed",
        ));
    }

    if let Some(text) = message.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            output.push(message_item(message_id(), text, "completed"));
        }
    }

    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            output.push(tool_call_item(tool_call)?);
        }
    }

    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);
    let status = response_status(finish_reason);

    Ok(json!({
        "id": response_id,
        "object": "response",
        "created_at": root
            .get("created")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| Utc::now().timestamp()),
        "status": status,
        "error": null,
        "incomplete_details": incomplete_details(finish_reason),
        "instructions": original_request.get("instructions").cloned().unwrap_or(Value::Null),
        "max_output_tokens": original_request.get("max_output_tokens").cloned().unwrap_or(Value::Null),
        "model": root
            .get("model")
            .cloned()
            .or_else(|| original_request.get("model").cloned())
            .unwrap_or(Value::Null),
        "output": output,
        "parallel_tool_calls": original_request
            .get("parallel_tool_calls")
            .cloned()
            .unwrap_or_else(|| json!(true)),
        "previous_response_id": original_request
            .get("previous_response_id")
            .cloned()
            .unwrap_or(Value::Null),
        "reasoning": original_request
            .get("reasoning")
            .cloned()
            .unwrap_or_else(|| json!({ "effort": null, "summary": null })),
        "store": original_request
            .get("store")
            .cloned()
            .unwrap_or_else(|| json!(false)),
        "temperature": original_request.get("temperature").cloned().unwrap_or(Value::Null),
        "text": original_request
            .get("text")
            .cloned()
            .unwrap_or_else(|| json!({ "format": { "type": "text" } })),
        "tool_choice": original_request
            .get("tool_choice")
            .cloned()
            .unwrap_or_else(|| json!("auto")),
        "tools": original_request
            .get("tools")
            .cloned()
            .unwrap_or_else(|| json!([])),
        "top_p": original_request.get("top_p").cloned().unwrap_or(Value::Null),
        "truncation": original_request
            .get("truncation")
            .cloned()
            .unwrap_or_else(|| json!("disabled")),
        "usage": map_usage(root.get("usage")),
        "user": original_request.get("user").cloned().unwrap_or(Value::Null),
        "metadata": original_request
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| json!({})),
    }))
}

#[derive(Debug, Clone)]
pub struct ChatToResponsesStream {
    response_id: String,
    message_id: String,
    created_at: i64,
    model: Value,
    original_request: Value,
    started: bool,
    opened_text_item: bool,
    text_output_index: Option<usize>,
    next_output_index: usize,
    reasoning: Option<StreamingReasoning>,
    text: String,
    tool_calls: BTreeMap<usize, StreamingToolCall>,
    usage: Option<Value>,
    completed: bool,
}

#[derive(Debug, Clone)]
struct StreamingToolCall {
    item_id: String,
    call_id: String,
    name: Option<String>,
    arguments: String,
    emitted_arguments_len: usize,
    output_index: usize,
    opened: bool,
}

#[derive(Debug, Clone)]
struct StreamingReasoning {
    item_id: String,
    output_index: usize,
    content: String,
    done: bool,
}

impl ChatToResponsesStream {
    pub fn new(original_request: Value) -> Self {
        let model = original_request
            .get("model")
            .cloned()
            .unwrap_or(Value::Null);
        Self {
            response_id: response_id(),
            message_id: message_id(),
            created_at: Utc::now().timestamp(),
            model,
            original_request,
            started: false,
            opened_text_item: false,
            text_output_index: None,
            next_output_index: 0,
            reasoning: None,
            text: String::new(),
            tool_calls: BTreeMap::new(),
            usage: None,
            completed: false,
        }
    }

    pub fn push_chat_chunk(&mut self, chunk: &Value) -> Result<Vec<ResponseStreamEvent>> {
        let mut events = Vec::new();
        if !self.started {
            events.extend(self.start_events());
            self.started = true;
        }

        if let Some(usage) = chunk.get("usage").filter(|v| !v.is_null()) {
            self.usage = Some(map_usage(Some(usage)));
        }

        let choices = chunk
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(ProxyTransformError::MissingField("choices"))?;

        for choice in choices {
            let choice = choice
                .as_object()
                .ok_or(ProxyTransformError::ExpectedObject("stream choice"))?;
            if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                if let Some(reasoning_delta) =
                    delta.get("reasoning_content").and_then(Value::as_str)
                {
                    events.extend(self.push_reasoning_delta(reasoning_delta));
                }
                if let Some(tool_call_deltas) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (fallback_index, tool_call_delta) in tool_call_deltas.iter().enumerate() {
                        events.extend(self.push_tool_call_delta(tool_call_delta, fallback_index)?);
                    }
                }
                if let Some(text_delta) = delta.get("content").and_then(Value::as_str) {
                    if !self.opened_text_item {
                        self.text_output_index = Some(self.allocate_output_index());
                        events.extend(self.open_text_events());
                        self.opened_text_item = true;
                    }
                    self.text.push_str(text_delta);
                    events.push(ResponseStreamEvent::new(
                        "response.output_text.delta",
                        json!({
                            "type": "response.output_text.delta",
                            "item_id": self.message_id,
                            "output_index": self.text_output_index.unwrap_or(0),
                            "content_index": 0,
                            "delta": text_delta,
                        }),
                    ));
                }
            }

            if choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .is_some()
                && !self.completed
            {
                events.extend(self.complete_events());
                self.completed = true;
            }
        }

        Ok(events)
    }

    fn push_reasoning_delta(&mut self, reasoning_delta: &str) -> Vec<ResponseStreamEvent> {
        if reasoning_delta.is_empty() {
            return Vec::new();
        }

        let mut events = Vec::new();
        if self.reasoning.is_none() {
            let output_index = self.allocate_output_index();
            let item_id = reasoning_id();
            events.push(ResponseStreamEvent::new(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": reasoning_item(item_id.clone(), None, "in_progress"),
                }),
            ));
            self.reasoning = Some(StreamingReasoning {
                item_id,
                output_index,
                content: String::new(),
                done: false,
            });
        }

        if let Some(reasoning) = &mut self.reasoning {
            reasoning.content.push_str(reasoning_delta);
        }
        events
    }

    fn push_tool_call_delta(
        &mut self,
        tool_call_delta: &Value,
        fallback_index: usize,
    ) -> Result<Vec<ResponseStreamEvent>> {
        let delta = tool_call_delta
            .as_object()
            .ok_or(ProxyTransformError::ExpectedObject(
                "stream tool call delta",
            ))?;
        let index = delta
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
            .unwrap_or(fallback_index);

        if !self.tool_calls.contains_key(&index) {
            let output_index = self.allocate_output_index();
            self.tool_calls.insert(
                index,
                StreamingToolCall {
                    item_id: function_call_id(),
                    call_id: delta
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .unwrap_or_else(call_id),
                    name: None,
                    arguments: String::new(),
                    emitted_arguments_len: 0,
                    output_index,
                    opened: false,
                },
            );
        }

        let mut events = Vec::new();
        let tool_call = self.tool_calls.get_mut(&index).expect("inserted above");
        if !tool_call.opened {
            if let Some(id) = delta.get("id").and_then(Value::as_str) {
                tool_call.call_id = id.to_string();
            }
        }

        let function = delta.get("function").and_then(Value::as_object);
        if let Some(name) = function
            .and_then(|function| function.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            tool_call.name = Some(name.to_string());
        }

        let argument_delta = function
            .and_then(|function| function.get("arguments"))
            .and_then(Value::as_str);

        if !tool_call.opened && tool_call.name.is_some() {
            events.push(ResponseStreamEvent::new(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": tool_call.output_index,
                    "item": function_call_item_from_parts(
                        tool_call.item_id.clone(),
                        tool_call.call_id.clone(),
                        tool_call.name.as_deref().unwrap_or_default(),
                        "",
                        "in_progress",
                    ),
                }),
            ));
            tool_call.opened = true;
        }

        if let Some(argument_delta) = argument_delta {
            tool_call.arguments.push_str(argument_delta);
        }

        if tool_call.opened && tool_call.emitted_arguments_len < tool_call.arguments.len() {
            let delta = tool_call.arguments[tool_call.emitted_arguments_len..].to_string();
            tool_call.emitted_arguments_len = tool_call.arguments.len();
            events.push(ResponseStreamEvent::new(
                "response.function_call_arguments.delta",
                json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": tool_call.item_id,
                    "output_index": tool_call.output_index,
                    "delta": delta,
                }),
            ));
        }

        Ok(events)
    }

    fn allocate_output_index(&mut self) -> usize {
        let index = self.next_output_index;
        self.next_output_index += 1;
        index
    }

    fn start_events(&self) -> Vec<ResponseStreamEvent> {
        let response = self.response_shell("in_progress", Vec::new(), Value::Null);
        vec![
            ResponseStreamEvent::new(
                "response.created",
                json!({
                    "type": "response.created",
                    "response": response,
                }),
            ),
            ResponseStreamEvent::new(
                "response.in_progress",
                json!({
                    "type": "response.in_progress",
                    "response": self.response_shell("in_progress", Vec::new(), Value::Null),
                }),
            ),
        ]
    }

    fn open_text_events(&self) -> Vec<ResponseStreamEvent> {
        let output_index = self.text_output_index.unwrap_or(0);
        vec![
            ResponseStreamEvent::new(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": self.message_id,
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                    },
                }),
            ),
            ResponseStreamEvent::new(
                "response.content_part.added",
                json!({
                    "type": "response.content_part.added",
                    "item_id": self.message_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": "",
                        "annotations": [],
                    },
                }),
            ),
        ]
    }

    fn complete_events(&mut self) -> Vec<ResponseStreamEvent> {
        let item = message_item(self.message_id.clone(), &self.text, "completed");
        let mut output_items = Vec::new();
        if let Some(reasoning) = &self.reasoning {
            output_items.push((
                reasoning.output_index,
                reasoning_item(
                    reasoning.item_id.clone(),
                    Some(encode_reasoning_content(&reasoning.content)),
                    "completed",
                ),
            ));
        }
        if self.opened_text_item {
            output_items.push((self.text_output_index.unwrap_or(0), item.clone()));
        }
        for tool_call in self.tool_calls.values() {
            if tool_call.opened {
                output_items.push((tool_call.output_index, completed_tool_call_item(tool_call)));
            }
        }
        output_items.sort_by_key(|(output_index, _)| *output_index);
        let output = output_items
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>();
        let usage = self.usage.clone().unwrap_or(Value::Null);

        let mut events = Vec::new();
        if let Some(reasoning) = &mut self.reasoning {
            if !reasoning.done {
                events.push(ResponseStreamEvent::new(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": reasoning.output_index,
                        "item": reasoning_item(
                            reasoning.item_id.clone(),
                            Some(encode_reasoning_content(&reasoning.content)),
                            "completed"
                        ),
                    }),
                ));
                reasoning.done = true;
            }
        }
        if self.opened_text_item {
            let output_index = self.text_output_index.unwrap_or(0);
            events.extend([
                ResponseStreamEvent::new(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "item_id": self.message_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "text": self.text,
                    }),
                ),
                ResponseStreamEvent::new(
                    "response.content_part.done",
                    json!({
                        "type": "response.content_part.done",
                        "item_id": self.message_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": {
                            "type": "output_text",
                            "text": self.text,
                            "annotations": [],
                        },
                    }),
                ),
                ResponseStreamEvent::new(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item,
                    }),
                ),
            ]);
        }
        for tool_call in self.tool_calls.values() {
            if !tool_call.opened {
                continue;
            }
            let item = completed_tool_call_item(tool_call);
            events.extend([
                ResponseStreamEvent::new(
                    "response.function_call_arguments.done",
                    json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": tool_call.item_id,
                        "output_index": tool_call.output_index,
                        "name": tool_call.name.as_deref().unwrap_or_default(),
                        "arguments": tool_call.arguments,
                    }),
                ),
                ResponseStreamEvent::new(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": tool_call.output_index,
                        "item": item,
                    }),
                ),
            ]);
        }

        events.push(ResponseStreamEvent::new(
            "response.completed",
            json!({
                "type": "response.completed",
                "response": self.response_shell("completed", output, usage),
            }),
        ));
        events
    }

    fn response_shell(&self, status: &str, output: Vec<Value>, usage: Value) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "error": null,
            "incomplete_details": null,
            "instructions": self
                .original_request
                .get("instructions")
                .cloned()
                .unwrap_or(Value::Null),
            "max_output_tokens": self
                .original_request
                .get("max_output_tokens")
                .cloned()
                .unwrap_or(Value::Null),
            "model": self.model,
            "output": output,
            "parallel_tool_calls": self
                .original_request
                .get("parallel_tool_calls")
                .cloned()
                .unwrap_or_else(|| json!(true)),
            "previous_response_id": self
                .original_request
                .get("previous_response_id")
                .cloned()
                .unwrap_or(Value::Null),
            "reasoning": self
                .original_request
                .get("reasoning")
                .cloned()
                .unwrap_or_else(|| json!({ "effort": null, "summary": null })),
            "store": self
                .original_request
                .get("store")
                .cloned()
                .unwrap_or_else(|| json!(false)),
            "temperature": self
                .original_request
                .get("temperature")
                .cloned()
                .unwrap_or(Value::Null),
            "text": self
                .original_request
                .get("text")
                .cloned()
                .unwrap_or_else(|| json!({ "format": { "type": "text" } })),
            "tool_choice": self
                .original_request
                .get("tool_choice")
                .cloned()
                .unwrap_or_else(|| json!("auto")),
            "tools": self
                .original_request
                .get("tools")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "top_p": self
                .original_request
                .get("top_p")
                .cloned()
                .unwrap_or(Value::Null),
            "truncation": self
                .original_request
                .get("truncation")
                .cloned()
                .unwrap_or_else(|| json!("disabled")),
            "usage": usage,
            "user": self.original_request.get("user").cloned().unwrap_or(Value::Null),
            "metadata": self
                .original_request
                .get("metadata")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }
}

fn message_item(id: String, text: &str, status: &str) -> Value {
    json!({
        "id": id,
        "type": "message",
        "status": status,
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": text,
            "annotations": [],
        }],
    })
}

fn reasoning_item(id: String, encrypted_content: Option<String>, status: &str) -> Value {
    let mut item = json!({
        "id": id,
        "type": "reasoning",
        "status": status,
        "summary": [],
    });
    if let Some(encrypted_content) = encrypted_content {
        item["encrypted_content"] = Value::String(encrypted_content);
    }
    item
}

fn tool_call_item(tool_call: &Value) -> Result<Value> {
    let obj = tool_call
        .as_object()
        .ok_or(ProxyTransformError::ExpectedObject("tool call"))?;
    if obj.get("type").and_then(Value::as_str) != Some("function") {
        return Err(ProxyTransformError::Unsupported(
            "only function tool calls can be mapped to Responses".to_string(),
        ));
    }
    let function = obj
        .get("function")
        .and_then(Value::as_object)
        .ok_or(ProxyTransformError::MissingField("tool_calls[].function"))?;
    let name =
        function
            .get("name")
            .and_then(Value::as_str)
            .ok_or(ProxyTransformError::MissingField(
                "tool_calls[].function.name",
            ))?;
    let arguments = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let call_id = obj
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(call_id);

    Ok(function_call_item_from_parts(
        function_call_id(),
        call_id,
        name,
        arguments,
        "completed",
    ))
}

fn completed_tool_call_item(tool_call: &StreamingToolCall) -> Value {
    function_call_item_from_parts(
        tool_call.item_id.clone(),
        tool_call.call_id.clone(),
        tool_call.name.as_deref().unwrap_or_default(),
        &tool_call.arguments,
        "completed",
    )
}

fn function_call_item_from_parts(
    id: String,
    call_id: String,
    name: &str,
    arguments: &str,
    status: &str,
) -> Value {
    json!({
        "id": id,
        "type": "function_call",
        "status": status,
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

fn map_usage(usage: Option<&Value>) -> Value {
    let Some(usage) = usage.and_then(Value::as_object) else {
        return Value::Null;
    };
    let input_tokens = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens + output_tokens);
    let cached_tokens = usage
        .get("prompt_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    json!({
        "input_tokens": input_tokens,
        "input_tokens_details": {
            "cached_tokens": cached_tokens,
        },
        "output_tokens": output_tokens,
        "output_tokens_details": {
            "reasoning_tokens": reasoning_tokens,
        },
        "total_tokens": total_tokens,
    })
}

fn response_status(finish_reason: Option<&str>) -> &'static str {
    match finish_reason {
        Some("length") | Some("content_filter") => "incomplete",
        _ => "completed",
    }
}

fn incomplete_details(finish_reason: Option<&str>) -> Value {
    match finish_reason {
        Some("length") => json!({ "reason": "max_output_tokens" }),
        Some("content_filter") => json!({ "reason": "content_filter" }),
        _ => Value::Null,
    }
}

fn response_id() -> String {
    format!("resp_{}", Uuid::new_v4().simple())
}

fn reasoning_id() -> String {
    format!("rs_{}", Uuid::new_v4().simple())
}

fn message_id() -> String {
    format!("msg_{}", Uuid::new_v4().simple())
}

fn function_call_id() -> String {
    format!("fc_{}", Uuid::new_v4().simple())
}

fn call_id() -> String {
    format!("call_{}", Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{chat_completion_to_response, ChatToResponsesStream};

    #[test]
    fn converts_chat_completion_to_response() {
        let original_request = json!({
            "model": "deepseek-chat",
            "input": "Hello",
            "instructions": "Be brief.",
            "max_output_tokens": 128,
        });
        let response = chat_completion_to_response(
            json!({
                "id": "chatcmpl_123",
                "object": "chat.completion",
                "created": 123,
                "model": "deepseek-chat",
                "choices": [{
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {
                        "role": "assistant",
                        "content": "Hi"
                    }
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 2,
                    "total_tokens": 12
                }
            }),
            &original_request,
        )
        .unwrap();

        assert_eq!(response["object"], "response");
        assert_eq!(response["status"], "completed");
        assert_eq!(response["output"][0]["type"], "message");
        assert_eq!(response["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(response["output"][0]["content"][0]["text"], "Hi");
        assert_eq!(response["usage"]["input_tokens"], 10);
        assert_eq!(response["usage"]["output_tokens"], 2);
    }

    #[test]
    fn converts_chat_tool_call_to_response_function_call() {
        let response = chat_completion_to_response(
            json!({
                "id": "chatcmpl_123",
                "created": 123,
                "model": "chat-model",
                "choices": [{
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_123",
                            "type": "function",
                            "function": {
                                "name": "list_files",
                                "arguments": "{\"path\":\".\"}"
                            }
                        }]
                    }
                }]
            }),
            &json!({ "model": "chat-model", "input": "list" }),
        )
        .unwrap();

        assert_eq!(response["output"][0]["type"], "function_call");
        assert_eq!(response["output"][0]["call_id"], "call_123");
        assert_eq!(response["output"][0]["name"], "list_files");
    }

    #[test]
    fn maps_chat_stream_text_to_response_events() {
        let mut mapper = ChatToResponsesStream::new(json!({
            "model": "chat-model",
            "input": "Hello"
        }));

        let events = mapper
            .push_chat_chunk(&json!({
                "id": "chatcmpl_123",
                "object": "chat.completion.chunk",
                "created": 123,
                "model": "chat-model",
                "choices": [{
                    "index": 0,
                    "delta": { "content": "Hi" },
                    "finish_reason": null
                }]
            }))
            .unwrap();
        assert!(events.iter().any(|e| e.event == "response.created"));
        assert!(events
            .iter()
            .any(|e| e.event == "response.output_text.delta"));

        let events = mapper
            .push_chat_chunk(&json!({
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 3,
                    "completion_tokens": 1,
                    "total_tokens": 4
                }
            }))
            .unwrap();
        assert!(events.iter().any(|e| e.event == "response.completed"));
        let completed = events
            .iter()
            .find(|e| e.event == "response.completed")
            .unwrap();
        assert_eq!(
            completed.data["response"]["output"][0]["content"][0]["text"],
            "Hi"
        );
        assert_eq!(completed.data["response"]["usage"]["total_tokens"], 4);
    }

    #[test]
    fn maps_chat_stream_tool_call_to_response_events() {
        let mut mapper = ChatToResponsesStream::new(json!({
            "model": "chat-model",
            "input": "pwd",
        }));

        let events = mapper
            .push_chat_chunk(&json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_exec",
                            "type": "function",
                            "function": {
                                "name": "exec_command",
                                "arguments": "{\"cmd\""
                            }
                        }]
                    },
                    "finish_reason": null
                }]
            }))
            .unwrap();
        assert!(events
            .iter()
            .any(|e| e.event == "response.output_item.added"
                && e.data["item"]["type"] == "function_call"));
        assert!(events
            .iter()
            .any(|e| e.event == "response.function_call_arguments.delta"));

        let events = mapper
            .push_chat_chunk(&json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "function": {
                                "arguments": ":\"pwd\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .unwrap();

        assert!(events
            .iter()
            .any(|e| e.event == "response.function_call_arguments.done"));
        let done_item = events
            .iter()
            .find(|e| e.event == "response.output_item.done")
            .unwrap();
        assert_eq!(done_item.data["item"]["call_id"], "call_exec");
        assert_eq!(done_item.data["item"]["name"], "exec_command");
        assert_eq!(done_item.data["item"]["arguments"], "{\"cmd\":\"pwd\"}");
        let completed = events
            .iter()
            .find(|e| e.event == "response.completed")
            .unwrap();
        assert_eq!(
            completed.data["response"]["output"][0]["type"],
            "function_call"
        );
        assert_eq!(
            completed.data["response"]["output"][0]["arguments"],
            "{\"cmd\":\"pwd\"}"
        );
    }

    #[test]
    fn maps_chat_reasoning_content_to_replayable_response_item() {
        let mut mapper = ChatToResponsesStream::new(json!({
            "model": "chat-model",
            "input": "pwd",
        }));

        let events = mapper
            .push_chat_chunk(&json!({
                "choices": [{
                    "index": 0,
                    "delta": { "reasoning_content": "Need to inspect cwd." },
                    "finish_reason": null
                }]
            }))
            .unwrap();
        assert!(events
            .iter()
            .any(|e| e.event == "response.output_item.added"
                && e.data["item"]["type"] == "reasoning"));

        let events = mapper
            .push_chat_chunk(&json!({
                "choices": [{
                    "index": 0,
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call_exec",
                            "type": "function",
                            "function": {
                                "name": "exec_command",
                                "arguments": "{\"cmd\":\"pwd\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .unwrap();

        let reasoning_done = events
            .iter()
            .find(|e| {
                e.event == "response.output_item.done" && e.data["item"]["type"] == "reasoning"
            })
            .unwrap();
        assert!(reasoning_done.data["item"]["encrypted_content"]
            .as_str()
            .unwrap()
            .starts_with("vibearound.reasoning.hex.v1:"));

        let completed = events
            .iter()
            .find(|e| e.event == "response.completed")
            .unwrap();
        assert_eq!(completed.data["response"]["output"][0]["type"], "reasoning");
        assert_eq!(
            completed.data["response"]["output"][1]["type"],
            "function_call"
        );
    }
}
