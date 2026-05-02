use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use crate::web_server::AppState;

/// GET /api/previews -- list all live preview sessions and the active tunnel URL.
pub async fn list_previews_handler(
    State(state): State<AppState>,
) -> Json<crate::api_types::PreviewsResponse> {
    let previews = common::previews::list_snapshots();
    let tunnel_url = state.tunnels.first_url();
    Json(crate::api_types::PreviewsResponse {
        previews,
        tunnel_url,
    })
}

/// DELETE /api/previews/:slug -- close one preview and kill its dev-server port.
pub async fn delete_preview_handler(Path(slug): Path<String>) -> impl IntoResponse {
    if common::previews::delete_session(&slug) {
        (StatusCode::OK, format!("Preview {} closed", slug))
    } else {
        (StatusCode::NOT_FOUND, format!("Preview {} not found", slug))
    }
}
