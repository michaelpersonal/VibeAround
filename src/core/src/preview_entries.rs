//! Short-lived preview entries for live preview URLs.
//!
//! Two modes:
//! - **Server mode**: `store_server(port, workspace, title)` — proxies to `localhost:{port}`
//! - **File mode**: `store_file(file, workspace, title)` — serves a file directly
//!   (markdown files get rendered client-side with marked.js + GitHub CSS)
//!
//! Entries expire after 5 minutes. Unlike pickup codes, lookups are multi-use —
//! the same slug can be accessed repeatedly within the TTL window.
//!
//! Cleanup: expired entries are purged on each `store*()` and `lookup()` call.
//! No background loop — stale entries are tiny and cleared on next access.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::rngs::OsRng;
use rand::Rng;

/// What the preview serves.
#[derive(Debug, Clone)]
pub enum PreviewKind {
    /// Reverse proxy to a running local server.
    Server { port: u16 },
    /// Serve a file directly (markdown rendered client-side).
    File { path: PathBuf },
}

/// Data associated with a live preview slug.
#[derive(Debug, Clone)]
pub struct PreviewEntry {
    pub kind: PreviewKind,
    pub workspace: PathBuf,
    pub title: String,
    pub created_at: Instant,
    pub expires_at: Instant,
}

static PREVIEW_ENTRIES: LazyLock<Mutex<HashMap<String, PreviewEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TTL: Duration = Duration::from_secs(300);

/// Character set for slugs: uppercase + digits, excluding ambiguous I/O/0/1.
const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate a cryptographically random 8-character slug.
fn generate_slug() -> String {
    let mut rng = OsRng;
    (0..8)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Store a server-mode preview and return the 8-char slug.
pub fn store_server(port: u16, workspace: PathBuf, title: String) -> String {
    store_entry(PreviewKind::Server { port }, workspace, title)
}

/// Store a file-mode preview and return the 8-char slug.
pub fn store_file(path: PathBuf, workspace: PathBuf, title: String) -> String {
    store_entry(PreviewKind::File { path }, workspace, title)
}

/// Internal: insert an entry with a fresh slug.
///
/// Retries generation on the rare case of a collision with an already-live
/// entry. The keyspace is ~31^8 ≈ 852 billion slugs against a handful of live
/// entries, so the loop terminates in a single iteration with overwhelming
/// probability.
fn store_entry(kind: PreviewKind, workspace: PathBuf, title: String) -> String {
    let mut map = PREVIEW_ENTRIES.lock();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);

    let slug = loop {
        let s = generate_slug();
        if !map.contains_key(&s) {
            break s;
        }
    };
    map.insert(
        slug.clone(),
        PreviewEntry {
            kind,
            workspace,
            title,
            created_at: now,
            expires_at: now + TTL,
        },
    );
    slug
}

/// Look up a preview entry by slug. Returns `None` if unknown or expired.
///
/// Multi-use: the entry is NOT consumed — repeated lookups succeed until TTL.
pub fn lookup(slug: &str) -> Option<PreviewEntry> {
    let mut map = PREVIEW_ENTRIES.lock();
    let now = Instant::now();
    map.retain(|_, e| e.expires_at > now);
    map.get(&slug.to_uppercase()).cloned()
}

/// Explicitly remove a preview entry (e.g. when the agent session ends).
pub fn remove(slug: &str) -> bool {
    let mut map = PREVIEW_ENTRIES.lock();
    map.remove(&slug.to_uppercase()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_slug_is_eight_chars_from_alphabet() {
        for _ in 0..100 {
            let slug = generate_slug();
            assert_eq!(slug.len(), 8);
            for b in slug.bytes() {
                assert!(CHARSET.contains(&b), "char {b:?} not in alphabet");
            }
        }
    }

    #[test]
    fn store_server_and_lookup_roundtrip() {
        let slug = store_server(3000, PathBuf::from("/tmp/project"), "My App".into());
        let entry = lookup(&slug).expect("slug should resolve");
        assert!(matches!(entry.kind, PreviewKind::Server { port: 3000 }));
        assert_eq!(entry.workspace, PathBuf::from("/tmp/project"));
        assert_eq!(entry.title, "My App");
    }

    #[test]
    fn store_file_and_lookup_roundtrip() {
        let slug = store_file(
            PathBuf::from("/tmp/project/README.md"),
            PathBuf::from("/tmp/project"),
            "README".into(),
        );
        let entry = lookup(&slug).expect("slug should resolve");
        match &entry.kind {
            PreviewKind::File { path } => assert_eq!(path, &PathBuf::from("/tmp/project/README.md")),
            _ => panic!("expected File kind"),
        }
    }

    #[test]
    fn lookup_is_multi_use() {
        let slug = store_server(8080, PathBuf::from("/home"), "Test".into());
        assert!(lookup(&slug).is_some());
        assert!(lookup(&slug).is_some(), "second lookup must also succeed");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let slug = store_server(9090, PathBuf::from("/root"), "CI".into());
        assert!(lookup(&slug.to_lowercase()).is_some());
    }

    #[test]
    fn unknown_slug_returns_none() {
        assert!(lookup("ZZZZZZZZ").is_none());
    }

    #[test]
    fn remove_deletes_entry() {
        let slug = store_server(4000, PathBuf::from("/tmp"), "Remove Test".into());
        assert!(remove(&slug));
        assert!(lookup(&slug).is_none());
    }

    #[test]
    fn remove_unknown_returns_false() {
        assert!(!remove("NONEXIST"));
    }
}
