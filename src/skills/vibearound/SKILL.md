---
name: vibearound
description: Hand over your current coding session so the user can continue the conversation on their phone or another device via any IM channel connected to VibeAround. Use when the user says "/vibearound handover", "hand over this session", "continue on my phone", or similar session transfer requests.
_vibearound:
  version: "${VERSION}"
  managed: true
---

# VibeAround Session Handover

Hand over the current coding session via the VibeAround orchestrator. The user can then pick it up from any connected IM channel (the pickup is not tied to a specific channel).

## When to Use

- User says `/vibearound handover`
- User asks to "hand over", "transfer", or "continue" the session on their phone or another device

## Prerequisites

The VibeAround MCP server must be connected (server name: `vibearound`). If not available, tell the user to start the VibeAround desktop app.

## Handover Steps

### 1. Resolve the session ID

```bash
cat ~/.claude/sessions/$PPID.json
```

Extract the `sessionId` field. If the file doesn't exist, inform the user.

### 2. Call prepare_handover

```
Tool: prepare_handover
Server: vibearound
Arguments:
  session_id: "<sessionId>"
  cwd: "<current working directory>"
  agent_kind: "claude"
```

If the tool says the workspace is not registered, ask the user for confirmation, then call `register_workspace` with the `cwd`, and retry.

### 3. Present the result

Show the `/pickup` command returned by the tool. The user sends it in any IM chat connected to VibeAround to resume the session there with the same agent.

## Error Handling

- **MCP server not available**: Start the VibeAround desktop app.
- **Workspace not registered**: Offer to register it (needs user confirmation).
- **Session ID not found**: Session metadata file may not exist.
