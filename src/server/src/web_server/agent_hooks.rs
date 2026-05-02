use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::agent_hooks::CodexHookEnvelope;

use super::AppState;

pub async fn codex_hook_handler(
    State(state): State<AppState>,
    Json(envelope): Json<CodexHookEnvelope>,
) -> StatusCode {
    if envelope.launch_id.trim().is_empty() {
        return StatusCode::BAD_REQUEST;
    }

    state.hook_registry.record_codex_hook(envelope);
    StatusCode::NO_CONTENT
}
