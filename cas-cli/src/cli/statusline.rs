//! Status line command for Claude Code integration
//!
//! Outputs a single-line status suitable for Claude Code's statusLine feature.
//! Designed to be fast and informative at a glance.

use std::path::Path;

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

use crate::cli::statusline::data_and_format::{collect_status_data, format_status_line};
use crate::types::AgentRole;

use crate::cli::Cli;

/// ANSI color codes for terminal output
mod colors {
    pub const RESET: &str = "\x1b[0m";
    pub const GREEN: &str = "\x1b[32m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const RED: &str = "\x1b[31m";
    pub const CYAN: &str = "\x1b[36m";
    pub const DIM: &str = "\x1b[2m";
    pub const BOLD: &str = "\x1b[1m";
}

#[derive(Parser)]
pub struct StatusLineArgs {
    /// Output raw JSON data instead of formatted line
    #[arg(long)]
    pub json: bool,

    /// Disable colors in output
    #[arg(long)]
    pub no_color: bool,

    /// Show minimal output (just counts)
    #[arg(long, short)]
    pub minimal: bool,

    /// Include session info from stdin (Claude Code passes JSON)
    #[arg(long)]
    pub with_session: bool,
}

/// Input from Claude Code's status line feature
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
struct ClaudeCodeInput {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<ModelInfo>,
    workspace: Option<WorkspaceInfo>,
    cost: Option<CostInfo>,
    context_window: Option<ContextWindowInfo>,
    rate_limits: Option<super::limits::ClaudeRateLimits>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ModelInfo {
    id: Option<String>,
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WorkspaceInfo {
    current_dir: Option<String>,
    project_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CostInfo {
    total_cost_usd: Option<f64>,
    total_duration_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ContextWindowInfo {
    total_input_tokens: Option<u64>,
    total_output_tokens: Option<u64>,
    context_window_size: Option<u64>,
}

/// Status line data structure for JSON output
#[derive(Debug, Serialize)]
pub struct StatusLineData {
    pub agents: AgentCounts,
    pub tasks: TaskCounts,
    pub memories: MemoryCounts,
    pub rules: RuleCounts,
    pub skills: SkillCounts,
    pub worktrees: WorktreeCounts,
    pub health: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,
    /// Multi-agent context: my tasks and other agents' work
    pub agent_context: AgentContext,
}

#[derive(Debug, Serialize)]
pub struct AgentCounts {
    pub active: usize,
    pub total: usize,
    pub claimed_tasks: usize,
}

#[derive(Debug, Serialize)]
pub struct TaskCounts {
    pub ready: usize,
    pub in_progress: usize,
    pub blocked: usize,
    pub claimed: usize,
    pub available: usize,
    pub total_open: usize,
    /// Tasks that are InProgress but have no active lease (orphaned/interrupted work)
    pub orphaned: usize,
}

#[derive(Debug, Serialize)]
pub struct MemoryCounts {
    pub total: usize,
    pub pinned: usize,
    pub helpful: usize,
    pub pending_extraction: usize,
}

#[derive(Debug, Serialize)]
pub struct RuleCounts {
    pub total: usize,
    pub proven: usize,
    pub stale: usize,
}

#[derive(Debug, Serialize)]
pub struct SkillCounts {
    pub total: usize,
    pub enabled: usize,
}

#[derive(Debug, Serialize)]
pub struct WorktreeCounts {
    pub active: usize,
    pub orphaned: usize,
    pub in_worktree: bool,
    pub current_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub daemon_running: bool,
    pub has_pending_work: bool,
    pub has_blocked_tasks: bool,
    pub status: String, // "ok", "pending", "blocked", "degraded"
}

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub latest_version: String,
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub model: Option<String>,
    pub context_pct: Option<u8>,
    /// Session ID for agent lookup
    #[serde(skip)]
    pub session_id: Option<String>,
}

/// Info about current agent's active task
#[derive(Debug, Clone, Serialize)]
pub struct MyTaskInfo {
    pub id: String,
    pub title: String,
    pub priority: i32,
    pub blocked_by: Vec<String>,
}

/// Info about other agents' work
#[derive(Debug, Clone, Serialize)]
pub struct OtherAgentWork {
    pub agent_id: String,
    pub agent_name: String,
    pub task_title: String,
}

/// Current agent context for multi-agent awareness
#[derive(Debug, Serialize)]
pub struct AgentContext {
    /// Current agent's ID (if identified via session)
    pub my_agent_id: Option<String>,
    /// Tasks I'm actively working on (claimed by me)
    pub my_tasks: Vec<MyTaskInfo>,
    /// Other agents that are actively working
    pub other_agents_working: Vec<OtherAgentWork>,
}

pub fn execute(args: &StatusLineArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    // Try to read Claude Code input from stdin (non-blocking)
    let session_info = if args.with_session {
        read_claude_code_input()
    } else {
        None
    };

    // In factory mode, skip the statusline entirely - the TUI handles status display
    // Check env var directly since agent may not be registered yet (MCP registration
    // happens on first tool call, but statusline hook runs earlier)
    if let Ok(role_str) = std::env::var("CAS_AGENT_ROLE") {
        if let Ok(role) = role_str.parse::<AgentRole>() {
            if matches!(
                role,
                AgentRole::Worker | AgentRole::Supervisor | AgentRole::Director
            ) {
                return Ok(());
            }
        }
    }

    // Collect status data
    let data = collect_status_data(session_info, cas_root.to_path_buf())?;

    if args.json || cli.json {
        println!("{}", serde_json::to_string(&data)?);
    } else {
        let line = format_status_line(&data, args.no_color, args.minimal);
        // cas-e1f4: always show which account this session uses.
        let account = dirs::home_dir().and_then(|home| {
            let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from);
            crate::accounts::active_claude_email(&home, config_dir.as_deref())
        });
        match account {
            Some(email) if args.no_color => writeln!(io::stdout(), "{email} | {line}")?,
            Some(email) => writeln!(
                io::stdout(),
                "{}{email}{} | {line}",
                colors::CYAN,
                colors::RESET
            )?,
            None => writeln!(io::stdout(), "{line}")?,
        }
    }

    Ok(())
}

/// The status line outside a Cassy project: just the account, when known.
pub fn execute_account_only(args: &StatusLineArgs) -> anyhow::Result<()> {
    if args.with_session { let _ = read_claude_code_input(); }
    if args.json {
        return Ok(());
    }
    let account = dirs::home_dir().and_then(|home| {
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from);
        crate::accounts::active_claude_email(&home, config_dir.as_deref())
    });
    if let Some(email) = account {
        writeln!(io::stdout(), "{email}")?;
    }
    Ok(())
}

fn read_claude_code_input() -> Option<SessionInfo> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).ok()?;
    parse_claude_code_input(&input)
}

fn parse_claude_code_input(input: &str) -> Option<SessionInfo> {
    let claude_input = serde_json::from_str::<ClaudeCodeInput>(input).ok()?;
    if let Some(limits) = claude_input.rate_limits {
        if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty()).map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".claude"))) {
            super::limits::record_claude_statusline(&dir, limits);
        }
    }
    let model = claude_input.model.and_then(|m| m.display_name);
    let context_pct = claude_input.context_window.and_then(|cw| {
        let size = cw.context_window_size?;
        let used = cw.total_input_tokens.unwrap_or(0) + cw.total_output_tokens.unwrap_or(0);
        (size > 0).then(|| ((used * 100) / size) as u8)
    });
    Some(SessionInfo { model, context_pct, session_id: claude_input.session_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_statusline_caches_only_usage_for_the_emitting_profile() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = env.home().join(".claude-fixture");
        std::fs::create_dir(&dir).unwrap();
        env.set("CLAUDE_CONFIG_DIR", dir.to_str().unwrap());
        let session = parse_claude_code_input(r#"{
            "session_id":"fixture-session", "model":{"display_name":"Opus"},
            "rate_limits":{"five_hour":{"used_percentage":23}},
            "secret_unrecognized_field":"must-not-be-cached"
        }"#).unwrap();
        // pin: native model.display_name still maps to the existing session output.
        assert_eq!(session.model.as_deref(), Some("Opus"));
        let usage = super::super::limits::account_usage(crate::accounts::Provider::Claude, &dir);
        assert_eq!(usage.windows[0].used_percent, Some(23.0));
        assert!(!usage.authenticated);
        assert!(!std::fs::read_to_string(dir.join(".cas-claude-usage.json")).unwrap().contains("must-not-be-cached"));
    }
}

mod data_and_format;
