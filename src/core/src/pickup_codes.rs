//! Short-lived pickup codes for session handover.
//!
//! `prepare_handover` generates a 4-char code mapping to (agent_kind, session_id, cwd).
//! The user pastes `/pickup <CODE>` in IM, and the code is consumed to resolve the
//! full handover parameters. Codes expire after 2 minutes.
//!
//! Cleanup: expired entries are purged on each `store()` and `consume()` call.
//! No background loop — stale entries are tiny and cleared on next access.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

struct PickupEntry {
    agent_kind: String,
    session_id: String,
    cwd: String,
    expires_at: Instant,
}

static PICKUP_CODES: LazyLock<Mutex<HashMap<String, PickupEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Simple counter mixed with timestamp to avoid collisions.
static COUNTER: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

const TTL: Duration = Duration::from_secs(120);

/// Character set for codes: uppercase + digits, excluding ambiguous I/O/0/1.
const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn generate_code() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut cnt = COUNTER.lock().unwrap();
    *cnt = cnt.wrapping_add(1);
    let seed = nanos ^ (*cnt as u128 * 2654435761); // mix counter with time
    (0..4)
        .map(|i| CHARSET[((seed >> (i * 8)) as usize) % CHARSET.len()] as char)
        .collect()
}

/// Store a pickup code and return the 4-char code string.
pub fn store(agent_kind: String, session_id: String, cwd: String) -> String {
    let mut map = PICKUP_CODES.lock().unwrap();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);

    // Retry if code collides with an existing live entry
    let code = loop {
        let c = generate_code();
        if !map.contains_key(&c) { break c; }
    };
    map.insert(code.clone(), PickupEntry {
        agent_kind,
        session_id,
        cwd,
        expires_at: now + TTL,
    });
    code
}

/// Look up and consume a pickup code. Returns (agent_kind, session_id, cwd) or None.
pub fn consume(code: &str) -> Option<(String, String, String)> {
    let mut map = PICKUP_CODES.lock().unwrap();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);
    let entry = map.remove(&code.to_uppercase())?;
    Some((entry.agent_kind, entry.session_id, entry.cwd))
}
