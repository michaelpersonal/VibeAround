//! Runtime state view of an `ACPPod`.

use agent_client_protocol as acp;

/// Mutable runtime fields of a pod. Consumers (dashboard, TUI, CLI) that
/// want a consistent view of the pod's current state call
/// `ACPPod::state().await` and get a clone of this struct.
///
/// Immutable fields (`route`, `started_at`, `bot_identity`) live directly
/// on `ACPPod` and are read without going through the state snapshot.
#[derive(Debug, Clone, Default)]
pub struct PodState {
    pub cli_kind: Option<String>,
    pub profile: Option<String>,
    pub session_id: Option<String>,
    pub workspace: Option<String>,
    pub busy: bool,
    pub failed: Option<String>,
    pub initialize: Option<acp::InitializeResponse>,
}
