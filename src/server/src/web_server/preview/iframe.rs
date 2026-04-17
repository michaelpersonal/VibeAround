//! Server-target preview: iframe wrapper + cookie setup.
//!
//! When a preview session has a `Server { port }` target, we render a
//! tiny HTML shell whose `<iframe src="/">` serves dev-server content
//! through the cookie-based proxy fallback at the root path. The slug
//! is stashed in a `va_preview` cookie with the same TTL as the share
//! key, so the proxy can look up the session on each request.

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

use common::preview_entries::{PreviewEntry, PreviewTarget};

use super::cookie_proxy::PREVIEW_COOKIE;
use super::markdown::render_md_page;
use super::toolbar::{escape_html, remaining_millis, toolbar_and_timer, TOOLBAR_CSS};

/// Look up a preview by slug and dispatch to the right renderer based on target.
pub(super) async fn render_preview(slug: &str) -> Result<Response, (StatusCode, String)> {
    let entry = common::preview_entries::lookup(slug).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Preview not found or expired.".to_string(),
        )
    })?;

    match entry.target {
        PreviewTarget::Server { port } => render_server_iframe(slug, &entry, port).await,
        PreviewTarget::File => render_md_page(&entry).await,
    }
}

/// Render the iframe wrapper for a Server preview; sets `va_preview` cookie
/// so the root-level cookie proxy can route requests to `localhost:{port}`.
async fn render_server_iframe(
    slug: &str,
    entry: &PreviewEntry,
    port: u16,
) -> Result<Response, (StatusCode, String)> {
    let remaining_ms = remaining_millis(entry);
    let remaining_secs = (remaining_ms / 1000) as u64;
    let title = escape_html(&entry.title);
    let subtitle = format!(":{}", port);

    let cookie_value = format!(
        "{}={}; Path=/; Max-Age={}; SameSite=Lax",
        PREVIEW_COOKIE, slug, remaining_secs
    );

    let toolbar = toolbar_and_timer(
        &title,
        &subtitle,
        remaining_ms,
        r#"<button onclick="document.querySelector('iframe').src=document.querySelector('iframe').src">Refresh</button>"#,
    );
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Preview — {title}</title>
<style>
  {toolbar_css}
  body {{ background: #111; }}
  iframe {{
    width: 100%;
    height: calc(100vh - 40px);
    border: 0;
    background: #fff;
  }}
</style>
</head>
<body>
{toolbar}
<iframe src="/"></iframe>
</body>
</html>"#,
        title = title,
        toolbar_css = TOOLBAR_CSS,
        toolbar = toolbar,
    );

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Set-Cookie", cookie_value)
        .body(Body::from(html))
        .unwrap())
}

pub(super) fn server_not_running_page(port: u16) -> Response {
    let html = format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Server not running</title>
<style>
  body {{ font-family: -apple-system, sans-serif; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; background: #f5f5f5; color: #333; }}
  .card {{ text-align: center; padding: 40px; background: #fff; border-radius: 12px; box-shadow: 0 2px 12px rgba(0,0,0,0.08); max-width: 400px; }}
  .card h2 {{ margin-bottom: 8px; }}
  .card p {{ color: #666; line-height: 1.5; }}
  .port {{ font-family: monospace; background: #eee; padding: 2px 6px; border-radius: 4px; }}
</style></head>
<body><div class="card">
  <h2>Server not running</h2>
  <p>No server is responding on port <span class="port">{port}</span>.<br>
  The dev server may have stopped or not started yet.</p>
</div></body></html>"#,
        port = port,
    );
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}
