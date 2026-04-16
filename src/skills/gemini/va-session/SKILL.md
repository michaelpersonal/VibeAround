---
name: va-session
description: Resolve your current session ID for use with other VibeAround tools. Called by other skills that need session context (e.g. va-preview, vibearound handover).
---

# VibeAround Session ID

Resolve your current Gemini CLI session ID. Other VibeAround skills reference this skill when they need session context.

## How to Resolve

Check recent sessions with `/resume` and use the most recent session ID for the current workspace. If you know the current session ID, use it directly.

If no session ID can be determined, return nothing — callers handle the missing case gracefully. The VibeAround server can also auto-discover Gemini sessions from `~/.gemini/projects.json`.
