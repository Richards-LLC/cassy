//! PPID-based agent identification
//!
//! Provides stable agent identification across `/compact`, `--resume`, and multi-terminal
//! scenarios by using the parent PID (Claude Code's PID) + machine hash.
//!
//! Agent ID format: `cc-{ppid}-{machine_hash}`
//! Example: `cc-12345-a8f3b2c1`

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
#[allow(unused_imports)]
use tracing::{debug, info, warn};

/// Compute a machine hash from hostname + username for cross-machine uniqueness
///
/// This ensures agent IDs are unique across machines when cloud-syncing.
pub fn compute_machine_hash() -> String {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();

    let mut hasher = DefaultHasher::new();
    hostname.hash(&mut hasher);
    username.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

/// Compute the agent ID for the MCP server context
///
/// Uses getppid() to get Claude Code's PID (since MCP is a subprocess),
/// combined with machine hash for uniqueness.
#[cfg(unix)]
pub fn compute_agent_id() -> String {
    let ppid = std::os::unix::process::parent_id();
    let machine_hash = compute_machine_hash();
    format!("cc-{ppid}-{machine_hash}")
}

/// Compute the agent ID for hook context
///
/// Hooks run as: Claude Code → shell → cas hook
/// So we need the grandparent PID (Claude Code), not the parent (shell).
///
/// Architecture:
/// ```text
/// Claude Code (PID C)
/// ├── MCP Server (PPID = C) → agent_id = cc-C-{hash}
/// └── shell (PPID = C)
///     └── cas hook Stop (PPID = shell, grandparent = C) → agent_id = cc-C-{hash}
/// ```
#[cfg(unix)]
pub fn compute_agent_id_for_hook() -> String {
    let ppid = std::os::unix::process::parent_id();
    let cc_pid = get_parent_of_pid(ppid).unwrap_or(ppid);
    let machine_hash = compute_machine_hash();
    format!("cc-{cc_pid}-{machine_hash}")
}

#[cfg(not(unix))]
pub fn compute_agent_id_for_hook() -> String {
    // On non-Unix, fall back to our own PID
    let pid = std::process::id();
    let machine_hash = compute_machine_hash();
    format!("cc-{}-{}", pid, machine_hash)
}

/// Get the parent PID of a given process (for subagent detection)
///
/// On Unix, reads /proc/{pid}/stat to extract PPID.
/// On macOS, uses `ps` command.
#[cfg(target_os = "linux")]
pub fn get_parent_of_pid(pid: u32) -> Option<u32> {
    // Read /proc/{pid}/stat and extract PPID (field 4)
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // Format: pid (comm) state ppid ...
    // We need to handle comm potentially containing spaces/parens
    // Find the last ')' to skip the comm field
    let last_paren = stat.rfind(')')?;
    let after_comm = &stat[last_paren + 2..]; // +2 to skip ") "
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // After ") state", the first field is ppid (index 1 since state is 0)
    fields.get(1)?.parse().ok()
}

#[cfg(target_os = "macos")]
pub fn get_parent_of_pid(pid: u32) -> Option<u32> {
    use std::process::Command;
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(windows)]
pub fn get_parent_of_pid(pid: u32) -> Option<u32> {
    // Windows implementation would use NtQueryInformationProcess or CreateToolhelp32Snapshot
    // For now, return None (subagent detection won't work on Windows)
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn get_parent_of_pid(_pid: u32) -> Option<u32> {
    None
}

/// Windows fallback for compute_agent_id
#[cfg(not(unix))]
pub fn compute_agent_id() -> String {
    // On non-Unix systems, fall back to using current PID
    // This is less stable but better than nothing
    let pid = std::process::id();
    let machine_hash = compute_machine_hash();
    format!("cc-{}-{}", pid, machine_hash)
}

/// Detect parent agent ID for subagent linking
///
/// When Claude Code spawns a subagent via Task tool:
/// - Parent CC (PID 12345) spawns Subagent CC (PID 23456)
/// - Subagent's MCP (PID 23457) has PPID = 23456
/// - We get grandparent PID (12345) and look up its agent ID
///
/// Returns the parent agent ID if:
/// 1. We can determine our grandparent PID
/// 2. That PID has a registered agent in the mapping file
pub fn detect_parent_agent_id(cas_root: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        let grandparent_pid = get_parent_of_pid(ppid)?;

        // Check if grandparent has registered agent
        let parent_file = cas_root.join(format!("agents_by_cc_pid/{grandparent_pid}"));
        std::fs::read_to_string(parent_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Register the agent's PID mapping for subagent detection
///
/// Writes the agent ID to a file keyed by Claude Code's PID so that
/// subagents can discover their parent agent.
pub fn register_agent_pid_mapping(cas_root: &Path, agent_id: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        let mapping_dir = cas_root.join("agents_by_cc_pid");
        std::fs::create_dir_all(&mapping_dir)?;
        std::fs::write(mapping_dir.join(ppid.to_string()), agent_id)?;
    }
    Ok(())
}

/// Cleanup the agent's PID mapping on shutdown
pub fn cleanup_agent_pid_mapping(cas_root: &Path) {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        let _ = std::fs::remove_file(cas_root.join(format!("agents_by_cc_pid/{ppid}")));
    }
}

/// Cleanup PID mapping for a specific Claude Code PID (used by hooks)
pub fn cleanup_pid_mapping_for_cc_pid(cas_root: &Path, cc_pid: u32) {
    let _ = std::fs::remove_file(cas_root.join(format!("agents_by_cc_pid/{cc_pid}")));
}

// ============================================================================
// Session ID Lookup (for auto-registration)
// ============================================================================
// PID → session mapping is maintained by the daemon in memory.
// SessionStart hook sends cc_pid to daemon, MCP queries daemon for session.

/// Read session_id for this MCP server (called by MCP on first tool use)
///
/// Queries the daemon via Unix socket for the session mapped to this Claude Code PID.
/// The daemon maintains PID → session mappings in memory (set by SessionStart hook).
pub fn read_session_for_mcp(cas_root: &Path) -> std::io::Result<String> {
    use crate::mcp::socket::{DaemonEvent, DaemonResponse, send_event};

    #[cfg(unix)]
    let cc_pid = std::os::unix::process::parent_id();
    #[cfg(not(unix))]
    let cc_pid = std::process::id();

    debug!(cc_pid = cc_pid, "Requesting session mapping for MCP");
    let event = DaemonEvent::GetSession { cc_pid };
    match send_event(cas_root, &event) {
        Ok(DaemonResponse::Session { session_id }) => {
            info!(
                cc_pid = cc_pid,
                session_id = %session_id,
                "Found session mapping for MCP"
            );
            Ok(session_id)
        }
        Ok(DaemonResponse::NoSession) => {
            warn!(cc_pid = cc_pid, "No session mapping found for MCP");
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No session found for PID {cc_pid}"),
            ))
        }
        Ok(other) => {
            warn!(cc_pid = cc_pid, response = ?other, "Unexpected daemon response");
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unexpected response: {other:?}"),
            ))
        }
        Err(e) => {
            warn!(cc_pid = cc_pid, error = %e, "Failed to query daemon for session mapping");
            Err(e)
        }
    }
}

/// Get the Claude Code PID (PPID in MCP context, our PID in hook context)
#[cfg(unix)]
pub fn get_cc_pid_for_mcp() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(not(unix))]
pub fn get_cc_pid_for_mcp() -> u32 {
    std::process::id()
}

/// Get the interactive harness PID from hook context (Claude / Codex / Grok).
///
/// Hooks can run in two modes:
/// 1. Direct: harness → cas hook (PPID = harness)
/// 2. Via shell: harness → shell → cas hook (grandparent = harness)
///
/// We detect which mode by checking whether the parent is a known harness
/// process (`claude`, `codex`, or `grok`). Pre-cas-3e56 this only matched
/// `"claude"`, so Grok SessionStart hooks walked up to the *grandparent*
/// (often a short-lived factory wrapper) and stored the wrong PID. The
/// daemon liveness gate then declared the agent dead while the real Grok
/// process was mid-turn — worker_status showed "None active".
#[cfg(unix)]
pub fn get_cc_pid_for_hook() -> u32 {
    let ppid = std::os::unix::process::parent_id();

    // Parent is the harness itself (direct hook spawn).
    if is_harness_process(ppid) {
        return ppid;
    }

    // Parent is a shell/wrapper — prefer grandparent when it is a harness.
    if let Some(gpid) = get_parent_of_pid(ppid) {
        if is_harness_process(gpid) {
            return gpid;
        }
    }

    // Fallback: parent if we cannot identify a harness (legacy behavior for
    // unknown wrappers). Prefer ppid over grandparent so we at least track
    // the immediate invoker rather than an unrelated ancestor.
    ppid
}

/// Whether `pid` looks like a supported interactive harness process.
///
/// Matches command names containing `claude`, `codex`, or `grok` (case-
/// insensitive). Public for tests and for callers that need the same
/// recognition as SessionStart PID stamping (cas-3e56).
#[cfg(target_os = "macos")]
pub fn is_harness_process(pid: u32) -> bool {
    use std::process::Command;
    if let Ok(output) = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
    {
        return comm_looks_like_harness(String::from_utf8_lossy(&output.stdout).trim());
    }
    false
}

#[cfg(target_os = "linux")]
pub fn is_harness_process(pid: u32) -> bool {
    // Prefer /proc/{pid}/comm; also check cmdline basename so renamed
    // binaries (e.g. `node` wrapping a harness) can still match argv0.
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        if comm_looks_like_harness(comm.trim()) {
            return true;
        }
    }
    if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
        // First NUL-separated token is argv0.
        let argv0 = cmdline
            .split(|&b| b == 0)
            .next()
            .map(|b| String::from_utf8_lossy(b))
            .unwrap_or_default();
        let base = std::path::Path::new(argv0.as_ref())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(argv0.as_ref());
        if comm_looks_like_harness(base) {
            return true;
        }
    }
    false
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn is_harness_process(_pid: u32) -> bool {
    false
}

/// Shared name matcher for harness process detection (cas-3e56).
pub(crate) fn comm_looks_like_harness(comm: &str) -> bool {
    let c = comm.to_ascii_lowercase();
    // Substring match: covers `claude`, `claude-code`, `grok`, full paths
    // already basenamed by callers, and `codex` / `codex-cli`.
    c.contains("claude") || c.contains("codex") || c.contains("grok")
}

/// Back-compat alias used by older call sites / tests.
#[cfg(any(test, unix))]
#[allow(dead_code)]
fn is_claude_code_process(pid: u32) -> bool {
    is_harness_process(pid)
}

#[cfg(not(unix))]
pub fn get_cc_pid_for_hook() -> u32 {
    std::process::id()
}

#[cfg(test)]
mod tests {
    use crate::agent_id::*;

    #[test]
    fn test_machine_hash_consistent() {
        let hash1 = compute_machine_hash();
        let hash2 = compute_machine_hash();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 8); // 8 hex chars
    }

    /// cas-3e56: harness recognition must cover Claude, Codex, and Grok so
    /// SessionStart stores the real interactive PID (not grandparent).
    #[test]
    fn comm_looks_like_harness_matches_all_supported_clis() {
        assert!(comm_looks_like_harness("claude"));
        assert!(comm_looks_like_harness("Claude"));
        assert!(comm_looks_like_harness("claude-code"));
        assert!(comm_looks_like_harness("codex"));
        assert!(comm_looks_like_harness("codex-cli"));
        assert!(comm_looks_like_harness("grok"));
        assert!(comm_looks_like_harness("Grok"));
        assert!(!comm_looks_like_harness("bash"));
        assert!(!comm_looks_like_harness("cargo"));
        assert!(!comm_looks_like_harness("node"));
    }

    #[test]
    fn test_agent_id_format() {
        let agent_id = compute_agent_id();
        assert!(agent_id.starts_with("cc-"));
        // Should be cc-{pid}-{8-char-hash}
        let parts: Vec<&str> = agent_id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "cc");
        assert!(parts[1].parse::<u32>().is_ok()); // PID
        assert_eq!(parts[2].len(), 8); // Hash
    }

    #[test]
    fn test_hook_agent_id_format() {
        let agent_id = compute_agent_id_for_hook();
        assert!(agent_id.starts_with("cc-"));
        let parts: Vec<&str> = agent_id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "cc");
        // In hook context, uses either:
        // - PPID if parent is Claude Code (direct execution)
        // - grandparent if parent is a shell (shell wrapper)
        // In test context, parent is cargo test (not Claude), so uses grandparent
        let pid_str = parts[1];
        let pid: u32 = pid_str.parse().unwrap();
        #[cfg(unix)]
        {
            // compute_agent_id_for_hook always uses grandparent (shell wrapper
            // model). get_cc_pid_for_hook is the harness-aware path (cas-3e56).
            let ppid = std::os::unix::process::parent_id();
            let expected = get_parent_of_pid(ppid).unwrap_or(ppid);
            assert_eq!(pid, expected);
        }
        #[cfg(not(unix))]
        {
            // On non-Unix, falls back to own PID
            assert_eq!(pid, std::process::id());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_get_parent_of_pid() {
        // Our own parent should be retrievable
        let our_pid = std::process::id();
        let our_ppid = std::os::unix::process::parent_id();

        // Test that we can get our own parent
        if let Some(ppid) = get_parent_of_pid(our_pid) {
            assert_eq!(ppid, our_ppid);
        }
        // Note: This might fail in some test environments, so we don't panic
    }

    #[test]
    fn test_pid_mapping() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path();

        let agent_id = "cc-12345-abcd1234";
        register_agent_pid_mapping(cas_root, agent_id).unwrap();

        // Verify the file was created
        #[cfg(unix)]
        {
            let ppid = std::os::unix::process::parent_id();
            let mapping_file = cas_root.join(format!("agents_by_cc_pid/{ppid}"));
            assert!(mapping_file.exists());
            let content = std::fs::read_to_string(&mapping_file).unwrap();
            assert_eq!(content, agent_id);
        }

        // Cleanup
        cleanup_agent_pid_mapping(cas_root);

        #[cfg(unix)]
        {
            let ppid = std::os::unix::process::parent_id();
            let mapping_file = cas_root.join(format!("agents_by_cc_pid/{ppid}"));
            assert!(!mapping_file.exists());
        }
    }

    #[test]
    fn test_subagent_parent_detection() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path();

        // Simulate parent agent registration by writing a PID mapping file
        // In real usage, the parent MCP writes this file keyed by its PPID (Claude Code's PID)
        let parent_agent_id = "cc-99999-deadbeef";
        let mapping_dir = cas_root.join("agents_by_cc_pid");
        std::fs::create_dir_all(&mapping_dir).unwrap();

        // Write a mapping for our grandparent PID (simulating parent agent)
        // In real subagent scenario:
        // - Parent CC (PID P) spawns Subagent CC (PID S)
        // - Subagent CC spawns MCP (PID M, PPID = S)
        // - MCP's grandparent = P, so it looks up agents_by_cc_pid/P
        #[cfg(unix)]
        {
            let our_ppid = std::os::unix::process::parent_id();
            // Simulate that our grandparent registered as an agent
            if let Some(grandparent) = get_parent_of_pid(our_ppid) {
                std::fs::write(mapping_dir.join(grandparent.to_string()), parent_agent_id).unwrap();

                // Now detect_parent_agent_id should find it
                let detected = detect_parent_agent_id(cas_root);
                assert_eq!(detected, Some(parent_agent_id.to_string()));

                // Cleanup
                std::fs::remove_file(mapping_dir.join(grandparent.to_string())).unwrap();
            }
        }

        // Test that detection returns None when no parent exists
        let no_parent = detect_parent_agent_id(cas_root);
        assert_eq!(no_parent, None);
    }

    #[test]
    fn test_cleanup_pid_mapping_for_specific_pid() {
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let cas_root = temp.path();

        let mapping_dir = cas_root.join("agents_by_cc_pid");
        std::fs::create_dir_all(&mapping_dir).unwrap();

        // Create a mapping file for a specific PID
        let test_pid: u32 = 12345;
        let mapping_file = mapping_dir.join(test_pid.to_string());
        std::fs::write(&mapping_file, "cc-12345-test1234").unwrap();
        assert!(mapping_file.exists());

        // Cleanup using the specific function
        cleanup_pid_mapping_for_cc_pid(cas_root, test_pid);
        assert!(!mapping_file.exists());
    }

    // Note: Session mapping tests removed - session lookup now uses daemon socket
    // and cannot be unit tested without a running daemon. Tested via integration tests.
}
