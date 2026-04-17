//! Process-group SIGKILL helpers for tracked dev-server ports.
//!
//! Resolution is entirely port-driven — we don't assume a specific runtime
//! (python / node / ruby / go / …). For each port we:
//!
//! 1. Find the listener PID via `lsof -ti :<port>`.
//! 2. Look up its process-group ID (`pgid`) via `ps -o pgid= -p <pid>`.
//! 3. SIGTERM the whole group, wait ~500ms, then SIGKILL any survivors.
//!
//! Why the process group instead of just the PID: agents commonly launch
//! dev servers through a shell wrapper (e.g. `sh -c "<cmd>"`). The listener
//! is the inner process; the shell is its parent in the same group. If we
//! SIGKILL only the listener, the shell keeps the pipe to the agent open,
//! the agent's output-watcher never sees EOF, and the current turn hangs
//! forever. Killing the group tears the whole wrapper tree down, the
//! watcher unblocks, and `acp::Agent::prompt` can return.

/// Kill every process *group* whose listener holds one of the given ports.
pub(super) fn kill_pids_on_ports(ports: &[u16]) {
    let pids = pids_listening_on(ports);
    if pids.is_empty() {
        return;
    }

    // PID → PGID (via `ps`). Deduplicate so we don't send the same signal twice.
    let pgids: std::collections::HashSet<i32> = pids
        .iter()
        .filter_map(|pid| pgid_for(*pid))
        .collect();

    if pgids.is_empty() {
        eprintln!(
            "[preview] kill: no process groups resolved for pids {:?}",
            pids
        );
        return;
    }

    #[cfg(unix)]
    {
        use std::process::Command;

        // First pass: SIGTERM. Gives the shell wrapper + agent watcher a
        // chance to unwind cleanly (flush stdout, emit SIGCHLD, etc.).
        for pgid in &pgids {
            let _ = Command::new("kill")
                .args(["-TERM", &format!("-{}", pgid)])
                .output();
            eprintln!("[preview] SIGTERM pgid={}", pgid);
        }

        // Give it half a second to exit politely, then SIGKILL survivors.
        std::thread::sleep(std::time::Duration::from_millis(500));

        for pgid in &pgids {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", pgid)])
                .output();
            eprintln!("[preview] SIGKILL pgid={}", pgid);
        }
    }

    #[cfg(not(unix))]
    {
        // Windows fallback: taskkill /T kills the process tree rooted at each PID.
        for pid in pids {
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .output();
        }
        let _ = pgids; // unused on non-unix
    }
}

/// Convenience wrapper for a single port.
pub(super) fn kill_port(port: u16) {
    kill_pids_on_ports(&[port]);
}

/// Resolve a PID to its process-group ID via `ps -o pgid= -p PID`.
#[cfg(unix)]
fn pgid_for(pid: u32) -> Option<i32> {
    let out = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()
}

#[cfg(not(unix))]
fn pgid_for(_pid: u32) -> Option<i32> {
    None
}

#[cfg(unix)]
fn pids_listening_on(ports: &[u16]) -> Vec<u32> {
    use std::process::Command;
    let mut pids = Vec::new();
    for port in ports {
        let out = match Command::new("lsof")
            .args(["-nP", "-ti", &format!("tcp:{}", port), "-sTCP:LISTEN"])
            .output()
        {
            Ok(o) => o,
            Err(_) => continue,
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(not(unix))]
fn pids_listening_on(_ports: &[u16]) -> Vec<u32> {
    // TODO: Windows via `netstat -ano` parsing.
    Vec::new()
}
