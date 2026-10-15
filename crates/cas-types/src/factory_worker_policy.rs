//! Explicit resource denials at the factory worker boundary.

use serde::{Deserialize, Serialize};

/// Project grants can narrow or authorize worker resources; these operator
/// denials always take precedence. Empty lists preserve the existing policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryWorkerPolicy {
    /// Explicit names authorized for worker CLI chores. Supervisor denials win.
    #[serde(default)]
    pub worker_credential_env: Vec<String>,
    #[serde(default)]
    pub supervisor_only_mcp: Vec<String>,
    #[serde(default)]
    pub supervisor_only_env: Vec<String>,
    /// cas-d757 (GH #1219): operator-authenticated CLIs a factory worker's
    /// shell may not run. Without credentials they start a login that pops
    /// up on the operator's screen. Matched on the executable's basename.
    #[serde(default = "default_worker_denied_clis")]
    pub worker_denied_clis: Vec<String>,
    /// cas-d757 (GH #1219): MCP/connector tool-name patterns a factory worker
    /// may not call. Case-insensitive; `*` matches any run of characters.
    #[serde(default = "default_worker_denied_mcp_tools")]
    pub worker_denied_mcp_tools: Vec<String>,
}

/// Default for [`FactoryWorkerPolicy::worker_denied_clis`].
pub fn default_worker_denied_clis() -> Vec<String> {
    vec!["vercel".to_string()]
}

/// Default for [`FactoryWorkerPolicy::worker_denied_mcp_tools`].
pub fn default_worker_denied_mcp_tools() -> Vec<String> {
    vec!["*vercel*".to_string()]
}

impl Default for FactoryWorkerPolicy {
    fn default() -> Self {
        Self {
            worker_credential_env: Vec::new(),
            supervisor_only_mcp: Vec::new(),
            supervisor_only_env: Vec::new(),
            worker_denied_clis: default_worker_denied_clis(),
            worker_denied_mcp_tools: default_worker_denied_mcp_tools(),
        }
    }
}

impl FactoryWorkerPolicy {
    pub fn denies_server(&self, name: &str) -> bool {
        self.supervisor_only_mcp
            .iter()
            .any(|candidate| candidate == name)
    }

    pub fn denies_env(&self, name: &str) -> bool {
        self.supervisor_only_env
            .iter()
            .any(|candidate| candidate == name)
    }

    /// cas-d757: whether `cli` (an executable's basename) is denied to workers.
    pub fn denies_cli(&self, cli: &str) -> bool {
        self.worker_denied_clis
            .iter()
            .any(|denied| denied.trim().eq_ignore_ascii_case(cli))
    }

    /// cas-d757: the first worker-denied pattern `tool` matches, if any.
    pub fn denied_mcp_tool_pattern(&self, tool: &str) -> Option<&str> {
        self.worker_denied_mcp_tools
            .iter()
            .map(|pattern| pattern.trim())
            .find(|pattern| !pattern.is_empty() && glob_matches(pattern, tool))
    }

    /// Whether the proxy needs native MCP scope isolation. The cas-d757
    /// worker denials are enforced by the pre-tool hook and do not count.
    pub fn is_empty(&self) -> bool {
        // Grants alone do not require native MCP scope isolation.
        self.supervisor_only_mcp.is_empty() && self.supervisor_only_env.is_empty()
    }
}

/// Case-insensitive glob with `*` as the only wildcard.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let text = text.to_ascii_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_denials_match_clis_and_tool_patterns_cas_d757() {
        let policy = FactoryWorkerPolicy::default();
        assert!(policy.denies_cli("vercel") && policy.denies_cli("Vercel"));
        assert!(!policy.denies_cli("vercel-helper") && !policy.denies_cli("git"));
        for tool in ["mcp__claude_ai_Vercel__list_projects", "mcp__vercel__deploy", "vercel"] {
            assert_eq!(policy.denied_mcp_tool_pattern(tool), Some("*vercel*"), "{tool}");
        }
        assert_eq!(policy.denied_mcp_tool_pattern("mcp__github__create_issue"), None);

        let custom = FactoryWorkerPolicy {
            worker_denied_mcp_tools: vec!["mcp__neon__*".into(), "*__deploy_*".into(), "exact".into()],
            ..FactoryWorkerPolicy::default()
        };
        assert!(custom.denied_mcp_tool_pattern("mcp__Neon__run_sql").is_some());
        assert!(custom.denied_mcp_tool_pattern("mcp__x__deploy_now").is_some());
        assert!(custom.denied_mcp_tool_pattern("EXACT").is_some());
        assert!(custom.denied_mcp_tool_pattern("exactly").is_none());
        assert!(custom.denied_mcp_tool_pattern("mcp__x__deploy").is_none());

        let open = FactoryWorkerPolicy {
            worker_denied_clis: Vec::new(),
            worker_denied_mcp_tools: Vec::new(),
            ..FactoryWorkerPolicy::default()
        };
        assert!(!open.denies_cli("vercel"));
        assert_eq!(open.denied_mcp_tool_pattern("mcp__vercel__deploy"), None);
        assert!(open.is_empty(), "the worker denials do not need proxy isolation");
    }
}
