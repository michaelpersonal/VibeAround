use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;

use chrono::Utc;
use common::config;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodexHookEnvelope {
    pub launch_id: String,
    pub profile_id: Option<String>,
    pub launch_target: Option<String>,
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodexSessionState {
    pub launch_id: String,
    pub profile_id: Option<String>,
    pub launch_target: Option<String>,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub last_event: String,
    pub last_turn_id: Option<String>,
    pub last_prompt: Option<String>,
    pub last_assistant_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Default)]
pub struct AgentHookRegistry {
    codex_sessions_by_launch: DashMap<String, CodexSessionState>,
}

impl AgentHookRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record_codex_hook(&self, envelope: CodexHookEnvelope) {
        let now = Utc::now().to_rfc3339();
        let session_id = string_field(&envelope.payload, "session_id");
        let transcript_path = string_field(&envelope.payload, "transcript_path");
        let cwd = string_field(&envelope.payload, "cwd");
        let model = string_field(&envelope.payload, "model");
        let source = string_field(&envelope.payload, "source");
        let turn_id = string_field(&envelope.payload, "turn_id");
        let prompt = string_field(&envelope.payload, "prompt");
        let last_assistant_message = string_field(&envelope.payload, "last_assistant_message");

        let mut state = self
            .codex_sessions_by_launch
            .entry(envelope.launch_id.clone())
            .or_insert_with(|| CodexSessionState {
                launch_id: envelope.launch_id.clone(),
                profile_id: envelope.profile_id.clone(),
                launch_target: envelope.launch_target.clone(),
                session_id: None,
                transcript_path: None,
                cwd: None,
                model: None,
                source: None,
                last_event: envelope.event.clone(),
                last_turn_id: None,
                last_prompt: None,
                last_assistant_message: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            });

        state.profile_id = state.profile_id.take().or(envelope.profile_id.clone());
        state.launch_target = state
            .launch_target
            .take()
            .or(envelope.launch_target.clone());
        state.session_id = session_id.or_else(|| state.session_id.take());
        state.transcript_path = transcript_path.or_else(|| state.transcript_path.take());
        state.cwd = cwd.or_else(|| state.cwd.take());
        state.model = model.or_else(|| state.model.take());
        state.source = source.or_else(|| state.source.take());
        state.last_event = envelope.event.clone();
        state.last_turn_id = turn_id.or_else(|| state.last_turn_id.take());
        state.last_prompt = prompt.or_else(|| state.last_prompt.take());
        state.last_assistant_message =
            last_assistant_message.or_else(|| state.last_assistant_message.take());
        state.updated_at = now;

        tracing::info!(
            launch_id = %envelope.launch_id,
            event = %envelope.event,
            session_id = ?state.session_id,
            transcript_path = ?state.transcript_path,
            "[agent-hooks] Codex hook event"
        );

        append_codex_event_jsonl(&envelope, &state);
    }

    pub fn codex_session_for_launch(&self, launch_id: &str) -> Option<CodexSessionState> {
        self.codex_sessions_by_launch
            .get(launch_id)
            .map(|entry| entry.value().clone())
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

fn append_codex_event_jsonl(envelope: &CodexHookEnvelope, state: &CodexSessionState) {
    let dir = config::data_dir().join("agent-hooks");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::debug!(error = %e, "failed to create agent hook log dir");
        return;
    }

    let line = serde_json::json!({
        "received_at": Utc::now().to_rfc3339(),
        "envelope": envelope,
        "state": state,
    });

    let path = dir.join("codex.jsonl");
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        tracing::debug!(path = ?path, "failed to open Codex hook log");
        return;
    };
    if let Err(e) = writeln!(file, "{line}") {
        tracing::debug!(error = %e, path = ?path, "failed to append Codex hook log");
    }
}
