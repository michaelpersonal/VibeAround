//! Short-lived pickup codes for session handover.
//!
//! `store()` generates a 4-char code mapping to (agent_kind, session_id, cwd).
//! The user pastes `/pickup <CODE>` in IM, and the code is consumed to resolve
//! the full handover parameters. Codes expire after 2 minutes.
//!
//! Cleanup: expired entries are purged on each `store()` and `consume()` call.
//! No background loop — stale entries are tiny and cleared on next access.
//!
//! Codes are generated with `OsRng` (cryptographically random) and retried on
//! collision. The 4-char length is kept for mobile-IM typing ergonomics; with
//! a 31-char alphabet that gives ~923 840 distinct codes, more than enough for
//! a single-user desktop app where at most a handful of codes are ever live.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::rngs::OsRng;
use rand::Rng;

struct PickupEntry {
    agent_kind: String,
    session_id: String,
    cwd: String,
    expires_at: Instant,
}

// SAFETY: std blocking Mutex held only for the duration of tiny in-memory
// map operations. Never hold this guard across an `.await`.
static PICKUP_CODES: LazyLock<Mutex<HashMap<String, PickupEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TTL: Duration = Duration::from_secs(120);

/// Character set for codes: uppercase + digits, excluding ambiguous I/O/0/1.
const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate a cryptographically random 4-character code.
fn generate_code() -> String {
    let mut rng = OsRng;
    (0..4)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Store a pickup code and return the 4-char code string.
///
/// Retries generation on the rare case of a collision with an already-live
/// entry. The keyspace is ~900k codes against a handful of live entries, so
/// the loop terminates in a single iteration with overwhelming probability.
pub fn store(agent_kind: String, session_id: String, cwd: String) -> String {
    let mut map = PICKUP_CODES.lock();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);

    let code = loop {
        let c = generate_code();
        if !map.contains_key(&c) {
            break c;
        }
    };
    map.insert(
        code.clone(),
        PickupEntry {
            agent_kind,
            session_id,
            cwd,
            expires_at: now + TTL,
        },
    );
    code
}

/// Look up and consume a pickup code. Returns (agent_kind, session_id, cwd) or None.
///
/// Consume-once: a successful call removes the entry, so replaying the same
/// code returns `None`.
pub fn consume(code: &str) -> Option<(String, String, String)> {
    let mut map = PICKUP_CODES.lock();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);
    let entry = map.remove(&code.to_uppercase())?;
    Some((entry.agent_kind, entry.session_id, entry.cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_is_four_chars_from_alphabet() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), 4);
            for b in code.bytes() {
                assert!(CHARSET.contains(&b), "char {b:?} not in alphabet");
            }
        }
    }

    #[test]
    fn store_and_consume_roundtrip() {
        let code = store("claude".into(), "sess-1".into(), "/tmp".into());
        let (agent, sess, cwd) = consume(&code).expect("code should resolve");
        assert_eq!(agent, "claude");
        assert_eq!(sess, "sess-1");
        assert_eq!(cwd, "/tmp");
    }

    #[test]
    fn consume_is_one_shot() {
        let code = store("gemini".into(), "sess-2".into(), "/home".into());
        assert!(consume(&code).is_some());
        assert!(consume(&code).is_none(), "second consume must fail");
    }

    #[test]
    fn consume_is_case_insensitive() {
        let code = store("codex".into(), "sess-3".into(), "/root".into());
        assert!(consume(&code.to_lowercase()).is_some());
    }

    #[test]
    fn unknown_code_returns_none() {
        assert!(consume("ZZZZ").is_none());
    }
}
