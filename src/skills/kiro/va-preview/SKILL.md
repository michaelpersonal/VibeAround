---
inclusion: always
name: va-preview
description: Start a live preview so the user can see your work in their browser or phone. Use after starting a dev server or creating HTML files.
---

# VibeAround Live Preview

Start a live preview so the user can see your work via a shareable URL.

## Steps

### 1. Start the server (if not already running)

### 2. Get your session ID

Use the `/va-session` skill to resolve your current session ID.

### 3. Call preview

```
Tool: preview
Server: vibearound
Arguments:
  port: <port>
  cwd: "<current working directory>"
  session_id: "<session_id from step 2>"  (pass if available)
  title: "<description>"  (optional)
```

### 4. Share the returned URLs with the user.

# VibeAround Live Preview

After you finish building a web application, HTML page, or any browsable artifact, start a live preview so the user can see the result immediately via a shareable URL.

## When to Use

- You just started a dev server (next dev, vite, python -m http.server, etc.)
- You created HTML/CSS/JS files the user should see
- The user asked to "show me", "preview", or "let me see it"
- Only when the VibeAround MCP server is connected

**Proactive behavior**: After starting a dev server or creating a web artifact, proactively ask the user if they'd like a preview link (e.g. "Want me to generate a preview link so you can see it on your phone?"). If the user confirms, call `preview`. Do NOT call the tool without asking first.

## Prerequisites

The VibeAround MCP server must be connected (server name: `vibearound`). If not available, tell the user to start the VibeAround desktop app.

## Steps

### 1. Start the server (if not already running)

Before calling preview, make sure:
- The port you want is free: `lsof -i :<port>` should return nothing
- The server is actually listening (wait for "Listening on..." or similar in the output)
- Use `--host 0.0.0.0` when available for broader compatibility

### 2. Call preview

```
Tool: preview
Server: vibearound
Arguments:
  port: <the port your server is running on>
  cwd: "<current working directory>"
  title: "<short description of what you built>"  (optional)
```

If the tool says the workspace is not registered, call `register_workspace` with the `cwd` first, then retry.

### 3. Share the URL

Include the returned URL in your reply. The user can tap it to see the live preview in their browser or phone. The link expires in 5 minutes.

## Error Handling

- **MCP server not available**: The VibeAround desktop app may not be running.
- **Workspace not registered**: Call `register_workspace` first, then retry.
- **Port in use**: Check with `lsof -i :<port>` and choose a different port.
