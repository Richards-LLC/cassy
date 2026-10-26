//! cas-d757 (GH #1219): operator-authenticated CLIs and MCP tools that a
//! factory worker may not use.
//!
//! A Codex worker on a Vercel-hosted project ran `vercel env ls production`.
//! With no CLI credentials it started a Vercel device login, which put an auth
//! prompt on the operator's screen, every time. The operator does not grant
//! it; the supervisor owns such access. This guard refuses a worker's shell
//! command that runs a CLI on `factory.worker_denied_clis` (defaults `vercel`, `neonctl`),
//! however wrapped, and an MCP or connector tool matching
//! `factory.worker_denied_mcp_tools` (defaults `*vercel*`, `*neon*`), including calls
//! dispatched through `mcp_execute`. The refusal sends the worker to the
//! supervisor with a blocker. Supervisors are never refused.
use std::path::Path;

use cas_core::hooks::types::HookInput;
use serde_json::Value;

use crate::config::{Config, load_global_config};

/// The two worker-denial lists of a factory configuration.
#[derive(Debug, Clone)]
struct WorkerDenials {
    clis: Vec<String>,
    mcp_tools: Vec<String>,
}

impl Default for WorkerDenials {
    fn default() -> Self {
        let factory = crate::config::FactoryConfig::default();
        Self { clis: factory.worker_denied_clis, mcp_tools: factory.worker_denied_mcp_tools }
    }
}

impl WorkerDenials {
    /// Whether `cli` (an executable's basename) is denied to workers.
    fn denies_cli(&self, cli: &str) -> bool {
        self.clis.iter().any(|denied| denied.trim().eq_ignore_ascii_case(cli))
    }

    /// The first denied pattern `tool` matches, if any.
    fn denied_mcp_tool_pattern(&self, tool: &str) -> Option<&str> {
        self.mcp_tools
            .iter()
            .map(|pattern| pattern.trim())
            .find(|pattern| !pattern.is_empty() && glob_matches(pattern, tool))
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
    if text.len() < first.len() + last.len() || !text.starts_with(first) || !text.ends_with(last) {
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

/// The refusal for a factory worker's tool call, or `None` when it may run.
pub(super) fn denial(input: &HookInput, tool: &str, cas_root: Option<&Path>) -> Option<String> {
    if !crate::harness_policy::is_factory_agent(input) || !crate::harness_policy::is_worker(input) {
        return None;
    }
    let policy = worker_policy(cas_root);
    let tool_input = input.tool_input.as_ref();
    if tool == "Bash" {
        let command = tool_input?.get("command")?.as_str()?;
        return denied_cli(&policy, command).map(|cli| cli_refusal(&cli));
    }
    denied_mcp_tool(&policy, tool, tool_input).map(|(name, pattern)| tool_refusal(&name, &pattern))
}

/// Refuse factory workers' access to operator-authenticated stores before
/// auto-approval. The path resolver is shared with every supported file and shell surface.
pub(super) fn credential_store_access_denial(input: &HookInput, tool: &str) -> Option<String> {
    if !crate::harness_policy::is_factory_agent(input) || !crate::harness_policy::is_worker(input) {
        return None;
    }
    if !super::operator_credentials::references_store(input, tool) {
        return None;
    }
    Some(operator_credential_refusal())
}

fn operator_credential_refusal() -> String {
    let prefix = crate::harness_policy::own_tool_prefix();
    format!(
        "🚫 WORKER-DENIED OPERATOR CREDENTIALS (GH #1293): factory workers cannot read the operator's Neon or Vercel authentication stores. Ask the supervisor via blocker=true: \
         `{prefix}coordination action=message target=supervisor blocker=true summary=\"need operator credential access\" \
         message=\"<what you need and why>\"`, then wait."
    )
}

/// The project's factory denials, else the global ones, else the defaults.
fn worker_policy(cas_root: Option<&Path>) -> WorkerDenials {
    cas_root
        .and_then(|root| Config::load(root).ok())
        .and_then(|config| config.factory)
        .or_else(|| load_global_config().factory)
        .map(|factory| WorkerDenials {
            clis: factory.worker_denied_clis,
            mcp_tools: factory.worker_denied_mcp_tools,
        })
        .unwrap_or_default()
}

/// The first denied CLI a shell command runs. Package runners count:
/// `npx vercel`, `bunx vercel`, `pnpm dlx vercel`, `yarn exec vercel` run it.
fn denied_cli(policy: &WorkerDenials, command: &str) -> Option<String> {
    super::pre_tool::shell_invocations(command)
        .into_iter()
        .find_map(|(name, args)| {
            let runs = package_runner_target(&name, &args).unwrap_or(name.as_str());
            let runs = runs.rsplit('/').next().unwrap_or(runs);
            // `npx vercel@latest`
            let runs = runs.split('@').next().filter(|cli| !cli.is_empty()).unwrap_or(runs);
            policy.denies_cli(runs).then(|| runs.to_string())
        })
}

/// The command a package runner executes, if `name` is one.
fn package_runner_target<'a>(name: &str, args: &'a [String]) -> Option<&'a str> {
    let rest = match name {
        "npx" | "bunx" | "pnpx" => args,
        "pnpm" | "yarn" | "npm" | "bun" => {
            let at = args
                .iter()
                .position(|arg| matches!(arg.as_str(), "dlx" | "exec" | "x"))?;
            &args[at + 1..]
        }
        _ => return None,
    };
    rest.iter()
        .map(String::as_str)
        .find(|arg| !arg.starts_with('-'))
}

/// The first denied tool a call names, with the pattern it matched. An
/// `mcp_execute` dispatch is judged by each `server.tool` it calls.
fn denied_mcp_tool(
    policy: &WorkerDenials,
    tool: &str,
    input: Option<&Value>,
) -> Option<(String, String)> {
    if let Some(pattern) = policy.denied_mcp_tool_pattern(tool) {
        return Some((tool.to_string(), pattern.to_string()));
    }
    if !tool.to_ascii_lowercase().ends_with("mcp_execute") {
        return None;
    }
    let mut calls = Vec::new();
    collect_dispatches(input?.get("code")?, &mut calls);
    calls.into_iter().find_map(|(server, method)| {
        let name = format!("mcp__{server}__{method}");
        policy
            .denied_mcp_tool_pattern(&name)
            .map(|pattern| (name.clone(), pattern.to_string()))
    })
}

/// `(server, tool)` pairs an `mcp_execute` code value dispatches: a JSON
/// object or array (possibly as a string), or `server.tool({...})` syntax.
fn collect_dispatches(code: &Value, calls: &mut Vec<(String, String)>) {
    match code {
        Value::Array(items) => items.iter().for_each(|item| collect_dispatches(item, calls)),
        Value::Object(call) => {
            let field = |key: &str| call.get(key).and_then(Value::as_str).unwrap_or_default();
            calls.push((field("server").to_string(), field("tool").to_string()));
        }
        Value::String(code) => {
            if let Ok(dispatch) = serde_json::from_str::<Value>(code) {
                return collect_dispatches(&dispatch, calls);
            }
            if let Some((server, rest)) = code.trim().split_once('.') {
                let method = rest.split_once('(').map_or(rest, |(method, _)| method);
                calls.push((server.trim().to_string(), method.trim().to_string()));
            }
        }
        _ => {}
    }
}

fn cli_refusal(cli: &str) -> String {
    let prefix = crate::harness_policy::own_tool_prefix();
    format!(
        "🚫 WORKER-DENIED CLI (GH #1219): factory workers do not run `{cli}`. Without credentials \
         it starts a login that prompts the operator, and the operator does not grant it; the \
         supervisor owns this access. Ask the supervisor via blocker=true: \
         `{prefix}coordination action=message target=supervisor blocker=true summary=\"need {cli}\" \
         message=\"<what you need from {cli} and why>\"`, then wait. The list is \
         factory.worker_denied_clis, which only the operator changes."
    )
}

fn tool_refusal(tool: &str, pattern: &str) -> String {
    let prefix = crate::harness_policy::own_tool_prefix();
    format!(
        "🚫 WORKER-DENIED TOOL (GH #1219): factory workers do not call `{tool}` (matches \
         `{pattern}`). Its service authenticates as the operator; the supervisor owns this \
         access. Ask the supervisor via blocker=true: \
         `{prefix}coordination action=message target=supervisor blocker=true summary=\"need {tool}\" \
         message=\"<what you need and why>\"`, then wait. The patterns are \
         factory.worker_denied_mcp_tools, which only the operator changes."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn worker_denials_match_clis_and_tool_patterns() {
        let policy = WorkerDenials::default();
        assert!(policy.denies_cli("vercel") && policy.denies_cli("Vercel"));
        assert!(!policy.denies_cli("vercel-helper") && !policy.denies_cli("git"));
        for tool in ["mcp__claude_ai_Vercel__list_projects", "mcp__vercel__deploy", "vercel"] {
            assert_eq!(policy.denied_mcp_tool_pattern(tool), Some("*vercel*"), "{tool}");
        }
        assert_eq!(policy.denied_mcp_tool_pattern("mcp__github__create_issue"), None);

        let custom = WorkerDenials {
            clis: Vec::new(),
            mcp_tools: vec!["mcp__neon__*".into(), "*__deploy_*".into(), "exact".into()],
        };
        assert!(!custom.denies_cli("vercel"), "an empty list disables the CLI check");
        assert!(custom.denied_mcp_tool_pattern("mcp__Neon__run_sql").is_some());
        assert!(custom.denied_mcp_tool_pattern("mcp__x__deploy_now").is_some());
        assert!(custom.denied_mcp_tool_pattern("EXACT").is_some());
        assert!(custom.denied_mcp_tool_pattern("exactly").is_none());
        assert!(custom.denied_mcp_tool_pattern("mcp__x__deploy").is_none());
    }

    #[test]
    fn denied_cli_sees_through_wrappers_and_package_runners() {
        let policy = WorkerDenials::default();
        for command in [
            "vercel env ls production",
            "/usr/local/bin/vercel whoami",
            "./node_modules/.bin/vercel deploy",
            "env VERCEL_ORG_ID=x vercel env pull",
            "VERCEL_TOKEN= vercel logs",
            "sudo -u deploy vercel ls",
            "timeout 30 vercel ls",
            "sh -c 'vercel env ls production'",
            "bash -lc \"cd web && vercel env ls\"",
            "eval vercel ls",
            "cd web && npx vercel env ls",
            "npx -y vercel@latest env ls",
            "pnpm dlx vercel ls",
            "yarn exec vercel ls",
            "git status; vercel ls | head",
        ] {
            assert_eq!(denied_cli(&policy, command).as_deref(), Some("vercel"), "{command}");
        }
        for command in [
            "git log --grep vercel",
            "grep -r vercel src/",
            "cat vercel.json",
            "echo vercel",
            "npm run build",
            "cas config get factory.worker_denied_clis",
        ] {
            assert_eq!(denied_cli(&policy, command), None, "{command}");
        }
    }

    #[test]
    fn denied_tools_cover_connectors_and_proxy_dispatches() {
        let policy = WorkerDenials::default();
        assert!(denied_mcp_tool(&policy, "mcp__claude_ai_Vercel__list_deployments", None).is_some());
        assert!(denied_mcp_tool(&policy, "mcp__vercel__get_project", None).is_some());
        assert!(denied_mcp_tool(&policy, "mcp__github__create_issue", None).is_none());
        for code in [
            json!("vercel.list_projects({})"),
            json!(r#"{"server":"vercel","tool":"deploy"}"#),
            json!([{"server":"github","tool":"get_issue"}, {"server":"vercel","tool":"env"}]),
        ] {
            assert!(
                denied_mcp_tool(&policy, "mcp__cs__mcp_execute", Some(&json!({"code": code})))
                    .is_some(),
                "{code}"
            );
        }
        assert!(
            denied_mcp_tool(
                &policy,
                "mcp__cas__mcp_execute",
                Some(&json!({"code": "github.get_issue({})"}))
            )
            .is_none()
        );
    }
}
