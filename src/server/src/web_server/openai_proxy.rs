use std::collections::VecDeque;
use std::io;
use std::pin::Pin;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};

use common::profiles::{catalog, normalize_legacy_profile, schema};

use crate::agent_hooks::CodexSessionState;
use crate::openai_proxy::{
    chat_completion_to_response, encode_sse_event,
    providers::{ProviderProxyAdapter, ProviderProxyContext},
    responses_to_chat_request, ChatToResponsesStream, ProxyTransformError,
};

use super::AppState;

type UpstreamByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static>>;

pub async fn responses_handler(
    State(state): State<AppState>,
    Path((profile_id, launch_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(original_request): Json<Value>,
) -> Response {
    responses_handler_inner(
        state,
        profile_id,
        Some(launch_id),
        headers,
        original_request,
    )
    .await
}

pub async fn legacy_responses_handler(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    headers: HeaderMap,
    Json(original_request): Json<Value>,
) -> Response {
    responses_handler_inner(state, profile_id, None, headers, original_request).await
}

async fn responses_handler_inner(
    state: AppState,
    profile_id: String,
    launch_id: Option<String>,
    headers: HeaderMap,
    original_request: Value,
) -> Response {
    let codex_session_state = launch_id
        .as_deref()
        .and_then(|launch_id| state.hook_registry.codex_session_for_launch(launch_id));
    let provider_context =
        provider_proxy_context(launch_id.as_deref(), codex_session_state.as_ref());
    let upstream_endpoint_result =
        upstream_chat_completions_endpoint(&profile_id, provider_context);
    let upstream_endpoint = match upstream_endpoint_result {
        Ok(endpoint) => endpoint,
        Err((status, message)) => return json_error(status, &message),
    };

    let mut chat_request = match responses_to_chat_request(original_request.clone()) {
        Ok(request) => request,
        Err(error) => return transform_error(error),
    };
    let mut provider_adapter = upstream_endpoint.provider_adapter;
    provider_adapter.prepare_chat_request(&original_request, &mut chat_request);
    log_proxy_exchange(
        &profile_id,
        launch_id.as_deref(),
        codex_session_state
            .as_ref()
            .and_then(|state| state.session_id.as_deref()),
        codex_session_state
            .as_ref()
            .and_then(|state| state.last_turn_id.as_deref()),
        &upstream_endpoint.url,
        &original_request,
        &chat_request,
    );
    let stream = chat_request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let auth = match authorization_header(&headers) {
        Some(auth) => auth,
        None => return json_error(StatusCode::UNAUTHORIZED, "missing Authorization header"),
    };

    let body = match serde_json::to_vec(&chat_request) {
        Ok(body) => body,
        Err(e) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to serialize proxy request: {e}"),
            );
        }
    };

    let upstream = match state
        .preview_client
        .post(upstream_endpoint.url)
        .header(reqwest::header::AUTHORIZATION, auth)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!("failed to reach upstream chat endpoint: {e}"),
            );
        }
    };

    if !upstream.status().is_success() {
        if proxy_debug_enabled() {
            eprintln!(
                "[va-openai-proxy] upstream_error profile={} status={}",
                profile_id,
                upstream.status()
            );
        }
        tracing::info!(
            target: "server::web_server::openai_proxy",
        profile_id = %profile_id,
        launch_id = ?launch_id,
        codex_session_id = ?codex_session_state.as_ref().and_then(|state| state.session_id.as_deref()),
        codex_turn_id = ?codex_session_state.as_ref().and_then(|state| state.last_turn_id.as_deref()),
        upstream_status = %upstream.status(),
        "OpenAI proxy upstream returned error"
        );
        return upstream_error_response(upstream).await;
    }

    tracing::info!(
        target: "server::web_server::openai_proxy",
        profile_id = %profile_id,
        launch_id = ?launch_id,
        codex_session_id = ?codex_session_state.as_ref().and_then(|state| state.session_id.as_deref()),
        codex_turn_id = ?codex_session_state.as_ref().and_then(|state| state.last_turn_id.as_deref()),
        upstream_status = %upstream.status(),
        stream = stream,
        "OpenAI proxy upstream accepted request"
    );
    if proxy_debug_enabled() {
        eprintln!(
            "[va-openai-proxy] upstream_ok profile={} status={} stream={}",
            profile_id,
            upstream.status(),
            stream
        );
    }

    if stream {
        stream_response(upstream, original_request, provider_adapter)
    } else {
        completion_response(upstream, original_request, provider_adapter).await
    }
}

struct UpstreamChatCompletionsEndpoint {
    url: String,
    provider_adapter: ProviderProxyAdapter,
}

fn provider_proxy_context(
    launch_id: Option<&str>,
    codex_session_state: Option<&CodexSessionState>,
) -> ProviderProxyContext {
    ProviderProxyContext {
        launch_id: launch_id.map(str::to_string),
        session_id: codex_session_state.and_then(|state| state.session_id.clone()),
        transcript_path: codex_session_state.and_then(|state| state.transcript_path.clone()),
    }
}

fn upstream_chat_completions_endpoint(
    profile_id: &str,
    provider_context: ProviderProxyContext,
) -> Result<UpstreamChatCompletionsEndpoint, (StatusCode, String)> {
    let profile = schema::load(profile_id)
        .map(normalize_legacy_profile)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("profile '{profile_id}' not found"),
            )
        })?;
    let provider = catalog::get(&profile.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("unknown provider '{}'", profile.provider),
        )
    })?;
    let endpoint = provider
        .endpoints
        .iter()
        .find(|endpoint| endpoint.api_type == "openai-chat")
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!(
                    "profile '{}' does not expose an OpenAI-compatible chat endpoint",
                    profile.id
                ),
            )
        })?;
    let base_url = profile
        .overrides
        .get("openai-chat")
        .and_then(|overrides| overrides.base_url.clone())
        .unwrap_or_else(|| endpoint.default_base_url.clone());
    let base_url = base_url.trim_end_matches('/');
    if base_url.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("profile '{}' has no chat base URL", profile.id),
        ));
    }
    Ok(UpstreamChatCompletionsEndpoint {
        url: format!("{base_url}/chat/completions"),
        provider_adapter: ProviderProxyAdapter::for_profile(&profile, provider_context),
    })
}

fn authorization_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

fn log_proxy_exchange(
    profile_id: &str,
    launch_id: Option<&str>,
    codex_session_id: Option<&str>,
    codex_turn_id: Option<&str>,
    upstream_url: &str,
    original_request: &Value,
    chat_request: &Value,
) {
    let message_count = chat_request
        .get("messages")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    tracing::info!(
        target: "server::web_server::openai_proxy",
        profile_id = %profile_id,
        launch_id = ?launch_id,
        codex_session_id = ?codex_session_id,
        codex_turn_id = ?codex_turn_id,
        upstream = %redacted_url(upstream_url),
        responses_model = %string_field(original_request, "model"),
        responses_stream = bool_field(original_request, "stream"),
        responses_tools = ?tool_types(original_request),
        responses_tool_choice = %tool_choice_summary(original_request.get("tool_choice")),
        chat_model = %string_field(chat_request, "model"),
        chat_stream = bool_field(chat_request, "stream"),
        chat_tools = ?chat_tool_names(chat_request),
        chat_tool_choice = %tool_choice_summary(chat_request.get("tool_choice")),
        message_count = message_count,
        "OpenAI proxy transformed Responses request to Chat request"
    );
    if proxy_debug_enabled() {
        eprintln!(
            "[va-openai-proxy] transform profile={} launch={:?} session={:?} turn={:?} upstream={} responses(model={}, stream={}, tools={:?}, tool_choice={}) -> chat(model={}, stream={}, tools={:?}, tool_choice={}, messages={})",
            profile_id,
            launch_id,
            codex_session_id,
            codex_turn_id,
            redacted_url(upstream_url),
            string_field(original_request, "model"),
            bool_field(original_request, "stream"),
            tool_types(original_request),
            tool_choice_summary(original_request.get("tool_choice")),
            string_field(chat_request, "model"),
            bool_field(chat_request, "stream"),
            chat_tool_names(chat_request),
            tool_choice_summary(chat_request.get("tool_choice")),
            message_count,
        );
    }
}

fn proxy_debug_enabled() -> bool {
    std::env::var_os("VIBEAROUND_PROXY_DEBUG").is_some()
}

fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("<unset>")
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn tool_types(request: &Value) -> Vec<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    tool.get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("<missing>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn chat_tool_names(request: &Value) -> Vec<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|tool| {
                    tool.get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("<non-function>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn tool_choice_summary(tool_choice: Option<&Value>) -> String {
    match tool_choice {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Object(obj)) => {
            if let Some(function_name) = obj
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
            {
                format!("function:{function_name}")
            } else {
                obj.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("<object>")
                    .to_string()
            }
        }
        Some(other) => format!("{other:?}"),
        None => "<unset>".to_string(),
    }
}

fn redacted_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

async fn completion_response(
    upstream: reqwest::Response,
    original_request: Value,
    mut provider_adapter: ProviderProxyAdapter,
) -> Response {
    let bytes = match upstream.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!("failed to read upstream response: {e}"),
            );
        }
    };
    let chat_response = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => value,
        Err(e) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                &format!("upstream returned invalid JSON: {e}"),
            );
        }
    };
    provider_adapter.observe_chat_completion(&chat_response);
    let response = match chat_completion_to_response(chat_response, &original_request) {
        Ok(response) => response,
        Err(error) => return transform_error(error),
    };
    Json(response).into_response()
}

fn stream_response(
    upstream: reqwest::Response,
    original_request: Value,
    provider_adapter: ProviderProxyAdapter,
) -> Response {
    let stream = map_chat_sse_to_responses(upstream, original_request, provider_adapter);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build proxy stream response",
            )
        })
}

async fn upstream_error_response(upstream: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
        .unwrap_or_else(|| "application/json".to_string());
    let body = match upstream.bytes().await {
        Ok(bytes) => Body::from(bytes),
        Err(e) => Body::from(
            json!({ "error": { "message": format!("failed to read upstream error body: {e}") } })
                .to_string(),
        ),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(body)
        .unwrap_or_else(|_| json_error(StatusCode::BAD_GATEWAY, "upstream request failed"))
}

fn transform_error(error: ProxyTransformError) -> Response {
    let status = match &error {
        ProxyTransformError::ExpectedObject(_) | ProxyTransformError::MissingField(_) => {
            StatusCode::BAD_REQUEST
        }
        ProxyTransformError::Unsupported(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    json_error(status, &error.to_string())
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "vibearound_proxy_error",
            }
        })),
    )
        .into_response()
}

fn map_chat_sse_to_responses(
    upstream: reqwest::Response,
    original_request: Value,
    provider_adapter: ProviderProxyAdapter,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static {
    let state = SseMapState {
        upstream: Box::pin(upstream.bytes_stream()),
        mapper: ChatToResponsesStream::new(original_request),
        provider_adapter,
        buffer: Vec::new(),
        queue: VecDeque::new(),
        done: false,
    };

    futures_util::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(item) = state.queue.pop_front() {
                return Some((item, state));
            }
            if state.done {
                return None;
            }

            match state.upstream.next().await {
                Some(Ok(chunk)) => state.ingest_chunk(&chunk),
                Some(Err(e)) => {
                    state.done = true;
                    return Some((
                        Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("upstream stream error: {e}"),
                        )),
                        state,
                    ));
                }
                None => return None,
            }
        }
    })
}

struct SseMapState {
    upstream: UpstreamByteStream,
    mapper: ChatToResponsesStream,
    provider_adapter: ProviderProxyAdapter,
    buffer: Vec<u8>,
    queue: VecDeque<Result<Bytes, io::Error>>,
    done: bool,
}

impl SseMapState {
    fn ingest_chunk(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
        while let Some(end) = find_sse_frame_end(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..end).collect();
            self.handle_frame(&frame);
            if self.done {
                break;
            }
        }
    }

    fn handle_frame(&mut self, frame: &[u8]) {
        let frame = String::from_utf8_lossy(frame);
        let data = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.trim().is_empty() {
            return;
        }
        if data.trim() == "[DONE]" {
            self.queue
                .push_back(Ok(Bytes::from_static(b"data: [DONE]\n\n")));
            self.done = true;
            return;
        }

        let chunk = match serde_json::from_str::<Value>(&data) {
            Ok(value) => value,
            Err(e) => {
                self.fail(format!("upstream sent invalid SSE JSON: {e}"));
                return;
            }
        };
        self.provider_adapter.observe_chat_stream_chunk(&chunk);
        let events = match self.mapper.push_chat_chunk(&chunk) {
            Ok(events) => events,
            Err(error) => {
                self.fail(error.to_string());
                return;
            }
        };
        for event in events {
            self.queue
                .push_back(Ok(Bytes::from(encode_sse_event(&event.event, &event.data))));
        }
    }

    fn fail(&mut self, message: String) {
        self.done = true;
        self.queue
            .push_back(Err(io::Error::new(io::ErrorKind::InvalidData, message)));
    }
}

fn find_sse_frame_end(buffer: &[u8]) -> Option<usize> {
    if buffer.len() < 2 {
        return None;
    }
    for index in 0..buffer.len() - 1 {
        if buffer[index] == b'\n' && buffer[index + 1] == b'\n' {
            return Some(index + 2);
        }
    }
    if buffer.len() < 4 {
        return None;
    }
    for index in 0..buffer.len() - 3 {
        if &buffer[index..index + 4] == b"\r\n\r\n" {
            return Some(index + 4);
        }
    }
    None
}
