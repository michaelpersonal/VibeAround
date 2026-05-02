use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseStreamEvent {
    pub event: String,
    pub data: Value,
}

impl ResponseStreamEvent {
    pub fn new(event: impl Into<String>, data: Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }
}

pub fn encode_sse_event(event: &str, data: &Value) -> String {
    let data = serde_json::to_string(data).unwrap_or_else(|_| "null".to_string());
    format!("event: {event}\ndata: {data}\n\n")
}
