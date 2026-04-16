---
name: va-session
description: Resolve your current session ID for use with other VibeAround tools. Called by other skills that need session context (e.g. va-preview, vibearound handover).
---

# VibeAround Session ID

Resolve your current session ID. Other VibeAround skills reference this skill when they need session context for lifecycle management.

## How to Resolve

Check your agent's session metadata. The method depends on which agent you are:

- **Claude Code**: Read `~/.claude/history.jsonl` (one JSON object per line). Find the last entry whose `project` field matches the current working directory. Extract its `sessionId` value.
- **Codex**: Read `~/.codex/history.jsonl` (one JSON object per line). Take the last line and extract its `session_id` value.
- **Gemini**: Check recent sessions with `/resume`. Use the most recent session ID for the current workspace.
- **Other agents**: If you have access to a session metadata file, extract the session ID. Otherwise, omit it — the server will attempt auto-discovery.

## Return Value

Return the session ID string to the calling skill. If no session ID can be found, return nothing — callers should handle the missing case gracefully.
