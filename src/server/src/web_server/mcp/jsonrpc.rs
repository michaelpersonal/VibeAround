//! JSON-RPC 2.0 envelope + MCP response helpers.

use axum::Json;

/// JSON-RPC 2.0 request envelope.
///
/// Marked `pub` because it is consumed by the axum `Json<JsonRpcRequest>`
/// extractor on `mcp_handler`, which is registered as a public route.
#[derive(serde::Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// Build a JSON-RPC 2.0 success response.
pub(super) fn jsonrpc_ok(
    id: Option<serde_json::Value>,
    result: serde_json::Value,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

/// Build a JSON-RPC 2.0 error response.
pub(super) fn jsonrpc_err(
    id: Option<serde_json::Value>,
    code: i64,
    message: &str,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
}

/// Build an MCP `tools/call` success envelope containing a text content block.
pub(super) fn mcp_text(id: Option<serde_json::Value>, text: &str) -> Json<serde_json::Value> {
    jsonrpc_ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": text }]
        }),
    )
}

/// Build an MCP `tools/call` error envelope (`isError: true`) containing a
/// text content block.
pub(super) fn mcp_error_text(
    id: Option<serde_json::Value>,
    text: &str,
) -> Json<serde_json::Value> {
    jsonrpc_ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "isError": true
        }),
    )
}
