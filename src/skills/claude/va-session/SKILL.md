---
name: va-session
description: Resolve your current session ID for use with other VibeAround tools. Called by other skills that need session context (e.g. va-preview, vibearound handover).
---

# VibeAround Session ID

Resolve your current Claude Code session ID. Other VibeAround skills reference this skill when they need session context.

## How to Resolve

Read `~/.claude/history.jsonl` (one JSON object per line). Find the last entry whose `project` field matches the current working directory, and extract its `sessionId` value.

If no match is found, return nothing — callers handle the missing case gracefully.
