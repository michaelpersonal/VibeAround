//! Cookie-based transparent proxy: the root `/` fallback handler.
//!
//! Once a preview iframe has set the `va_preview` cookie pointing at a
//! slug, every sub-resource request the iframe makes lands at `/` on the
//! dashboard server. This handler looks up the slug, proxies to the dev
//! server on `localhost:{port}` (trying IPv4 then IPv6 loopback), and
//! forwards most response headers except the framing-related ones that
//! would break the iframe.
//!
//! Direct top-level navigation to any proxied path is explicitly blocked
//! — preview content must only be accessed inside the iframe wrapper.
//! Unknown / missing `Sec-Fetch-Dest` values are treated as a direct
//! navigation (fail-closed).

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};

use common::preview_entries::PreviewTarget;

use crate::web_server::AppState;

use super::iframe::server_not_running_page;

/// Cookie name used to route root-level requests to the dev server.
pub(super) const PREVIEW_COOKIE: &str = "va_preview";

/// Cookie name for authenticated owner sessions (set by /pair flow).
pub(super) const OWNER_COOKIE: &str = "va_owner";

/// Fallback handler for root `/` — the cookie-based dev-server proxy.
///
/// Security rules:
/// - `/va/*` paths → serve dashboard SPA (never proxy)
/// - Has cookie + `Sec-Fetch-Dest: document` (direct navigation) → redirect to /va/
/// - Has cookie + iframe/sub-resource context → proxy to dev server
/// - No cookie → redirect to `/va/`
pub async fn cookie_proxy_fallback(State(state): State<AppState>, req: Request) -> Response {
    // Never proxy /va/ paths — they belong to the dashboard.
    let path = req.uri().path();
    if path == "/va" || path.starts_with("/va/") {
        return crate::web_server::spa_fallback(state.dist_for_fallback.clone()).await;
    }

    // Check Sec-Fetch-Dest: only allow iframe and sub-resource contexts.
    // Direct top-level navigation (or missing header, e.g. stripped by tunnel
    // proxy) is blocked. This is an allowlist — unknown values are rejected.
    let sec_fetch_dest = req
        .headers()
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("document"); // missing header → treat as direct navigation

    let is_allowed_context = matches!(
        sec_fetch_dest,
        "iframe" | "script" | "style" | "image" | "font" | "audio" | "video"
            | "empty"    // fetch() / XHR
            | "worker"   // Web Worker / Service Worker
            | "manifest" // Web App Manifest
    );
    let is_direct_navigation = !is_allowed_context;

    // Extract slug from cookie.
    let slug = extract_cookie(&req, PREVIEW_COOKIE);

    let slug = match slug {
        Some(s) => s,
        None => return Redirect::temporary("/va/").into_response(),
    };

    // Block direct navigation — preview content must only be accessed
    // inside an iframe wrapper. Redirect to dashboard instead of error page.
    if is_direct_navigation {
        return Redirect::temporary("/va/").into_response();
    }

    let entry = match common::preview_entries::lookup(&slug) {
        Some(e) => e,
        None => {
            // Cookie exists but entry expired — clear cookie and redirect.
            let clear_cookie = format!("{}=; Path=/; Max-Age=0; SameSite=Lax", PREVIEW_COOKIE);
            return Response::builder()
                .status(StatusCode::FOUND)
                .header("Location", "/va/")
                .header("Set-Cookie", clear_cookie)
                .body(Body::empty())
                .unwrap();
        }
    };

    let port = match &entry.target {
        PreviewTarget::Server { port } => *port,
        PreviewTarget::File => return preview_error_page(),
    };

    // Proxy the request to the dev server.
    let sub_path = req.uri().path().trim_start_matches('/');
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    // Try IPv4 first, then IPv6 loopback.
    let urls = [
        format!("http://127.0.0.1:{}/{}{}", port, sub_path, query),
        format!("http://[::1]:{}/{}{}", port, sub_path, query),
    ];

    let method = req.method().clone();
    let mut upstream_resp = None;

    for url in &urls {
        let upstream_req = state.preview_client.request(method.clone(), url);
        match upstream_req.send().await {
            Ok(resp) => {
                upstream_resp = Some(resp);
                break;
            }
            Err(e) if e.is_connect() => continue,
            Err(e) => {
                return (StatusCode::BAD_GATEWAY, format!("Upstream error: {e}")).into_response();
            }
        }
    }

    let upstream = match upstream_resp {
        Some(r) => r,
        None => return server_not_running_page(port),
    };

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let mut builder = Response::builder().status(status);

    // Forward safe response headers, strip framing-related ones.
    for (key, val) in upstream.headers() {
        let name = key.as_str().to_lowercase();
        match name.as_str() {
            // Strip headers that would break iframe embedding or leak info.
            "x-frame-options" | "content-security-policy" | "strict-transport-security" => {}
            // Forward everything else.
            _ => {
                builder = builder.header(key, val);
            }
        }
    }

    let body = match upstream.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read upstream body: {e}"),
            )
                .into_response();
        }
    };

    builder.body(Body::from(body)).unwrap()
}

/// Extract a named cookie value from the request.
pub(super) fn extract_cookie(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .map(|s| s.trim())
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k.trim() == name {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
}

/// Error page shown when a user directly navigates to a proxied path
/// outside of the iframe wrapper. Styled to match the dashboard's
/// Unauthorized page.
fn preview_error_page() -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(Body::from(preview_error_html()))
        .unwrap()
}

fn preview_error_html() -> String {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Preview — Access Denied</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    display: flex; align-items: center; justify-content: center;
    min-height: 100vh; padding: 24px;
    background: #09090b; color: #fafafa;
  }
  .card { max-width: 420px; width: 100%; }
  .badge {
    display: flex; align-items: center; gap: 12px;
    margin-bottom: 24px;
  }
  .badge .icon {
    width: 40px; height: 40px; border-radius: 8px;
    background: rgba(59, 130, 246, 0.1); color: #3b82f6;
    display: flex; align-items: center; justify-content: center;
  }
  .badge h1 { font-size: 16px; font-weight: 600; }
  .badge p { font-size: 12px; color: #71717a; }
  .info {
    border: 1px solid #27272a; border-radius: 8px;
    background: rgba(24, 24, 27, 0.5); padding: 16px;
    font-size: 14px; color: #a1a1aa; line-height: 1.6;
    margin-bottom: 24px;
  }
  .info p + p { margin-top: 12px; }
  .hint {
    font-size: 12px; font-weight: 500; text-transform: uppercase;
    letter-spacing: 0.05em; color: #71717a; margin-bottom: 8px;
  }
  ol { padding-left: 20px; font-size: 14px; color: rgba(250,250,250,0.9); }
  ol li { margin-bottom: 4px; }
  code { font-family: ui-monospace, monospace; font-size: 12px; }
  .footer {
    text-align: center; font-size: 10px; color: rgba(113,113,122,0.6);
    margin-top: 24px;
  }
</style>
</head>
<body>
<div class="card">
  <div class="badge">
    <div class="icon">
      <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24"
           fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="10"/>
        <line x1="12" y1="8" x2="12" y2="12"/>
        <line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
    </div>
    <div>
      <h1>Preview not available</h1>
      <p>This page can only be viewed inside a VibeAround preview frame.</p>
    </div>
  </div>
  <div class="info">
    <p>
      You've navigated directly to a preview proxy URL.
      For security, preview content is only accessible through the
      VibeAround preview iframe wrapper.
    </p>
    <p>
      If you had a preview link, it may have expired (links are valid for 5 minutes).
    </p>
  </div>
  <p class="hint">How to preview</p>
  <ol>
    <li>Ask your coding agent to run <code>preview</code> with the dev server port.</li>
    <li>Open the link the agent provides.</li>
  </ol>
  <p class="footer">VibeAround · preview links expire after 5 minutes</p>
</div>
</body>
</html>"#.to_string()
}
