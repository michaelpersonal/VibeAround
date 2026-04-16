---
name: va-session
description: Resolve your current session ID for use with other VibeAround tools. Called by other skills that need session context (e.g. va-preview, vibearound handover).
---

# VibeAround Session ID

Resolve your current Codex session ID. Other VibeAround skills reference this skill when they need session context.

## How to Resolve

Read `~/.codex/history.jsonl` (one JSON object per line). Take the last line and extract its `session_id` value.

If the file doesn't exist or is empty, return nothing — callers handle the missing case gracefully.
