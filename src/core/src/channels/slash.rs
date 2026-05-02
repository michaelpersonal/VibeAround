//! Slash command parsing.
//!
//! Converts raw prompt text into a `SlashAction` variant. Handles
//! IM-client quirks (line wraps, extra whitespace, `/va <rest>` aliases for
//! Slack, `/agent <rest>` passthrough for sending commands into the
//! downstream agent CLI).

/// A parsed system slash command. `None` means the text was a regular
/// prompt, not a slash command — the caller should forward it to the agent.
pub(crate) enum SlashAction {
    /// `/agent <rest>` — strip prefix, send rest as a slash command to the
    /// agent CLI (e.g. `/agent status` → `/status`).
    AgentPassthrough(String),
    /// `/new` — reset session (new conversation, same agent).
    NewSession,
    /// `/switch <agent_kind>` — switch agent.
    SwitchAgent(String),
    /// `/profile <profile>` — switch profile.
    SwitchProfile(String),
    /// `/close` — close route.
    Close,
    /// `/help` or `/commands` — show system command menu.
    ShowCommandMenu,
    /// `/agent` (no args) — list available agent commands.
    ListAgentCommands,
    /// `/pickup <agent_kind> <session_id> [cwd]` — import a session from a
    /// coding agent (Direction 1, legacy full command).
    Pickup {
        agent_kind: String,
        session_id: String,
        cwd: Option<String>,
    },
    /// `/pickup <CODE>` — short code lookup.
    PickupCode(String),
    /// `/pair <CODE>` — pair a browser with this VibeAround instance.
    Pair(String),
    /// `/handover` — export current session to a coding agent (Direction 2).
    Handover,
    /// `/plan` — switch the current session to plan mode (no tool execution).
    /// Equivalent to `/mode plan`, kept as a shorthand.
    PlanMode,
    /// `/mode <modeId>` — switch session permission mode.
    /// Supported: default, plan, acceptEdits, bypassPermissions, dontAsk.
    SetMode(String),
    /// Unknown slash command.
    Unknown(String),
}

/// Strip IM line-wrapping: remove `\r\n` / `\n` / `\r` plus any trailing
/// spaces after them. IM clients sometimes convert line breaks to spaces or
/// sprinkle extra whitespace around user-entered commands.
fn strip_line_wraps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' || c == '\n' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            while chars.peek().is_some_and(|c| *c == ' ') {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a slash command from prompt text. Returns `None` if the text is
/// not a slash command (regular prompt → forward to agent).
pub(crate) fn parse_slash_command(text: &str) -> Option<SlashAction> {
    // Pre-process: strip IM line-wraps, then collapse runs of spaces into one.
    let cleaned = strip_line_wraps(text);
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    // /va <rest> and /vibearound <rest> — Slack-friendly aliases.
    // Strip the prefix and re-parse the rest as if user typed /<rest>.
    //   "/va help"          → "/help"
    //   "/va switch claude" → "/switch claude"
    //   "/va agent status"  → "/agent status" → agent passthrough
    for prefix in ["/va ", "/vibearound "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.is_empty() {
                return Some(SlashAction::ShowCommandMenu);
            }
            let reparsed = if rest.starts_with('/') {
                rest.to_string()
            } else {
                format!("/{}", rest)
            };
            return parse_slash_command(&reparsed);
        }
    }
    if trimmed == "/va" || trimmed == "/vibearound" {
        return Some(SlashAction::ShowCommandMenu);
    }

    // /agent <rest> — passthrough to agent CLI as a slash command.
    // Accepts: /agent status, /agent /status, /agent/status — all become "/status".
    if let Some(rest) = trimmed.strip_prefix("/agent/") {
        let rest = rest.trim();
        if !rest.is_empty() {
            return Some(SlashAction::AgentPassthrough(format!("/{}", rest)));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/agent ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            // Strip leading slash if present — we always add one
            let cmd = rest.strip_prefix('/').unwrap_or(rest);
            return Some(SlashAction::AgentPassthrough(format!("/{}", cmd)));
        }
    }
    if trimmed == "/agent" {
        return Some(SlashAction::ListAgentCommands);
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim().to_string());

    match cmd {
        "/new" => Some(SlashAction::NewSession),
        "/switch" => match arg {
            Some(kind) if !kind.is_empty() => Some(SlashAction::SwitchAgent(kind)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        "/profile" => match arg {
            Some(profile) if !profile.is_empty() => Some(SlashAction::SwitchProfile(profile)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        "/close" => Some(SlashAction::Close),
        "/help" | "/commands" => Some(SlashAction::ShowCommandMenu),
        "/pickup" => {
            // /pickup <CODE>                          — short code
            // /pickup <agent_kind> <session_id> [cwd] — legacy full form
            match arg {
                Some(rest) if !rest.is_empty() => {
                    let parts: Vec<&str> = rest.splitn(3, ' ').collect();
                    if parts.len() == 1 {
                        Some(SlashAction::PickupCode(parts[0].to_string()))
                    } else if parts.len() >= 2 && !parts[1].is_empty() {
                        let cwd = parts
                            .get(2)
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        Some(SlashAction::Pickup {
                            agent_kind: parts[0].to_string(),
                            session_id: parts[1].to_string(),
                            cwd,
                        })
                    } else {
                        Some(SlashAction::Unknown(trimmed.to_string()))
                    }
                }
                _ => Some(SlashAction::Unknown(trimmed.to_string())),
            }
        }
        "/handover" => Some(SlashAction::Handover),
        "/plan" => Some(SlashAction::PlanMode),
        "/mode" => match arg {
            Some(mode) if !mode.is_empty() => Some(SlashAction::SetMode(mode)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        "/pair" => match arg {
            Some(code) if !code.is_empty() => Some(SlashAction::Pair(code)),
            _ => Some(SlashAction::Unknown(trimmed.to_string())),
        },
        _ => Some(SlashAction::Unknown(trimmed.to_string())),
    }
}
