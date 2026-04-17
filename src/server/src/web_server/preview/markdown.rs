//! Markdown-target preview: rendered document page.
//!
//! When a preview session has a `File` target (currently always markdown),
//! render the file into a standalone page using GitHub markdown CSS +
//! client-side `marked.js` parsing. The source file is escaped into a JS
//! template literal so we avoid re-rendering on every request.

use axum::http::StatusCode;
use axum::response::Response;

use common::preview_entries::{PreviewEntry, PreviewTarget};

use super::toolbar::{escape_html, html_response, remaining_millis, toolbar_and_timer, TOOLBAR_CSS};

/// Render a markdown file preview with toolbar + GitHub CSS + marked.js.
pub(super) async fn render_md_page(
    entry: &PreviewEntry,
) -> Result<Response, (StatusCode, String)> {
    let file_path = match &entry.target {
        PreviewTarget::File => &entry.id,
        PreviewTarget::Server { .. } => {
            return Err((
                StatusCode::BAD_REQUEST,
                "This preview serves a server, not a file.".to_string(),
            ));
        }
    };

    let content = tokio::fs::read_to_string(file_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read file: {e}"),
        )
    })?;

    let remaining_ms = remaining_millis(entry);
    let title = escape_html(&entry.title);

    // Build subtitle: workspace_name / relative_path (or just filename)
    let ws_name = entry
        .workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let subtitle = if let Ok(rel) = file_path.strip_prefix(&entry.workspace) {
        format!(
            "{} / {}",
            escape_html(ws_name),
            escape_html(&rel.display().to_string())
        )
    } else {
        escape_html(ws_name).to_string()
    };

    // Escape the markdown for embedding in a JS template literal.
    let escaped_md = content
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
        .replace("</script>", "<\\/script>");

    let toolbar = toolbar_and_timer(&title, &subtitle, remaining_ms, "");
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/github-markdown-css@5/github-markdown-light.min.css">
<style>
  {toolbar_css}
  body {{
    background: #fff;
    color: #1f2328;
  }}
  .markdown-body {{
    max-width: 880px;
    margin: 0 auto;
    padding: 32px 24px 64px;
  }}
  @media (max-width: 767px) {{
    .markdown-body {{ padding: 16px; }}
  }}
  .markdown-body pre {{
    background: #f6f8fa;
    border-radius: 6px;
    padding: 16px;
    overflow-x: auto;
  }}
  .markdown-body code {{
    font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
    font-size: 85%;
  }}
  .markdown-body pre code {{
    background: transparent;
    padding: 0;
  }}
  .markdown-body table {{
    border-collapse: collapse;
    width: 100%;
  }}
  .markdown-body table th,
  .markdown-body table td {{
    border: 1px solid #d0d7de;
    padding: 6px 13px;
  }}
  .markdown-body table tr:nth-child(2n) {{
    background: #f6f8fa;
  }}
</style>
</head>
<body>
{toolbar}
<article class="markdown-body" id="content"></article>
<script src="https://cdn.jsdelivr.net/npm/marked@15/marked.min.js"></script>
<script>
(function() {{
  var raw = `{raw_md}`;
  document.getElementById('content').innerHTML = marked.parse(raw, {{ gfm: true }});
}})();
</script>
</body>
</html>"#,
        title = title,
        toolbar_css = TOOLBAR_CSS,
        toolbar = toolbar,
        raw_md = escaped_md,
    );
    Ok(html_response(html))
}
