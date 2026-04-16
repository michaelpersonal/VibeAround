---
name: va-session
description: Resolve your current session ID for use with other VibeAround tools. Called by other skills that need session context (e.g. va-preview, vibearound handover).
---

# VibeAround Session ID

Resolve your current Qwen Code session ID. Other VibeAround skills reference this skill when they need session context.

## How to Resolve

If you have access to a session metadata file or know the current session ID, use it. Otherwise, omit it — the VibeAround server will attempt auto-discovery.
