//! Configuration traits for hooks
//!
//! Defines traits that abstract over configuration access, allowing
//! the hooks module to work with different configuration backends.

use serde::{Deserialize, Serialize};

/// Configuration for plan mode context building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeConfig {
    /// Whether to use plan-aware context in plan mode
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Token budget for plan mode (typically higher than execution)
    #[serde(default = "default_plan_token_budget")]
    pub token_budget: usize,

    /// Maximum tasks to show in plan context
    #[serde(default = "default_plan_task_limit")]
    pub task_limit: usize,

    /// Include dependency trees in plan context
    #[serde(default = "default_true")]
    pub show_dependencies: bool,

    /// Include closed tasks in dependency context
    #[serde(default)]
    pub include_closed: bool,
}

impl Default for PlanModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            token_budget: default_plan_token_budget(),
            task_limit: default_plan_task_limit(),
            show_dependencies: true,
            include_closed: false,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_plan_token_budget() -> usize {
    12000
}
fn default_plan_task_limit() -> usize {
    20
}
fn default_token_budget() -> usize {
    4000
}
fn default_ai_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

/// Configuration trait for hooks
///
/// Abstracts over configuration access so the hooks module can work
/// with different configuration backends (CLI config, MCP config, etc.)
pub trait HooksConfig {
    /// Token budget for context injection
    fn token_budget(&self) -> usize;

    /// Whether to start with minimal context (context engineering pattern)
    fn minimal_start(&self) -> bool;

    /// Whether MCP mode is enabled
    fn mcp_enabled(&self) -> bool;

    /// Whether to use AI-powered context selection
    fn ai_context(&self) -> bool;

    /// Model to use for AI context selection
    fn ai_model(&self) -> String;

    /// Whether to fall back to non-AI context if AI fails
    fn ai_fallback(&self) -> bool;

    /// Whether context injection is enabled
    fn inject_context(&self) -> bool;

    /// Get plan mode configuration
    fn plan_mode(&self) -> PlanModeConfig;

    /// Get supervisor role guidance content
    fn supervisor_guidance(&self) -> String {
        default_supervisor_guidance()
    }

    /// Get worker role guidance content
    fn worker_guidance(&self) -> String {
        default_worker_guidance()
    }
}

/// Default supervisor guidance content (fallback when cas-cli builtins are unavailable)
fn default_supervisor_guidance() -> String {
    r#"
# Factory Supervisor

You coordinate workers to complete EPICs. You are a planner, not an implementer.

## Hard Rules
- **Never implement tasks yourself.** Delegate ALL coding to workers.
- **Never close tasks for workers.** Workers own their closes.
- **Never poll or sleep.** The system is event-driven. After assigning tasks, end your turn and wait.

## Workflow
1. Create EPIC: `mcp__cas__task action=create task_type=epic title="..." description="..."`
2. Break into subtasks, group by file overlap to prevent merge conflicts
3. Spawn workers: `mcp__cas__coordination action=spawn_workers count=N`
4. Assign tasks and send context: `mcp__cas__task action=update id=<id> assignee=<worker>`
5. Wait for worker messages (completion, blockers, questions)
6. Merge completed work to epic branch, tell workers to sync
7. When all tasks done, merge epic to base branch and cleanup worktrees"#
        .to_string()
}

/// Default worker guidance content (fallback when cas-cli builtins are unavailable)
fn default_worker_guidance() -> String {
    r#"
# Factory Worker

You execute tasks assigned by the Supervisor. You may be in an isolated worktree or sharing the main directory.

## Worktree Mode
Check on first turn: `[[ "$PWD" == *".cas/worktrees"* ]] && echo "WORKTREE" || echo "MAIN"`
If WORKTREE: MCP tools may be slow (SQLite contention). Try once, if unavailable go to Fallback Workflow.
NEVER run `cas init`, `cas factory`, or any `cas` CLI command in worktrees.

## Tool Availability
Try `mcp__cas__task action=mine` once. If it fails, use Fallback Workflow — do NOT retry.

## Workflow
1. Check assignments: `mcp__cas__task action=mine`
2. Start a task: `mcp__cas__task action=start id=<task-id>`
3. Read task details and understand acceptance criteria before coding
4. Implement, committing after each logical unit of work
5. Report progress: `mcp__cas__task action=notes id=<task-id> notes="..." note_type=progress`
6. When done: attempt `mcp__cas__task action=close id=<task-id> reason="..."`
   - If verification-required: message supervisor immediately, do NOT retry or spawn verifiers

## Communication
Primary: `mcp__cas__coordination action=message target=supervisor summary="<one line>" message="<response>"`
Fallback (if MCP unavailable): use SendMessage with to: "supervisor"

Report blockers immediately:
`mcp__cas__task action=update id=<task-id> status=blocked`"#
        .to_string()
}

/// Default hooks configuration
///
/// Used when no configuration is available. Provides sensible defaults.
#[derive(Debug, Clone, Default)]
pub struct DefaultHooksConfig {
    pub token_budget: usize,
    pub minimal_start: bool,
    pub mcp_enabled: bool,
    pub ai_context: bool,
    pub ai_model: String,
    pub ai_fallback: bool,
    pub inject_context: bool,
    pub plan_mode: PlanModeConfig,
}

impl DefaultHooksConfig {
    pub fn new() -> Self {
        Self {
            token_budget: default_token_budget(),
            minimal_start: false,
            mcp_enabled: false,
            ai_context: false,
            ai_model: default_ai_model(),
            ai_fallback: true, // Always fall back by default
            inject_context: true,
            plan_mode: PlanModeConfig::default(),
        }
    }

    /// Create config with MCP mode enabled
    pub fn with_mcp(mut self) -> Self {
        self.mcp_enabled = true;
        self
    }

    /// Create config with specified token budget
    pub fn with_token_budget(mut self, budget: usize) -> Self {
        self.token_budget = budget;
        self
    }
}

impl HooksConfig for DefaultHooksConfig {
    fn token_budget(&self) -> usize {
        self.token_budget
    }

    fn minimal_start(&self) -> bool {
        self.minimal_start
    }

    fn mcp_enabled(&self) -> bool {
        self.mcp_enabled
    }

    fn ai_context(&self) -> bool {
        self.ai_context
    }

    fn ai_model(&self) -> String {
        self.ai_model.clone()
    }

    fn ai_fallback(&self) -> bool {
        self.ai_fallback
    }

    fn inject_context(&self) -> bool {
        self.inject_context
    }

    fn plan_mode(&self) -> PlanModeConfig {
        self.plan_mode.clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::hooks::config::*;

    #[test]
    fn test_default_config() {
        let config = DefaultHooksConfig::new();
        assert_eq!(config.token_budget(), 4000);
        assert!(!config.minimal_start());
        assert!(!config.mcp_enabled());
        assert!(config.inject_context());
    }

    #[test]
    fn test_with_mcp() {
        let config = DefaultHooksConfig::new().with_mcp();
        assert!(config.mcp_enabled());
    }

    #[test]
    fn test_with_token_budget() {
        let config = DefaultHooksConfig::new().with_token_budget(8000);
        assert_eq!(config.token_budget(), 8000);
    }

    #[test]
    fn test_plan_mode_default() {
        let plan = PlanModeConfig::default();
        assert!(plan.enabled);
        assert_eq!(plan.token_budget, 12000);
        assert_eq!(plan.task_limit, 20);
        assert!(plan.show_dependencies);
        assert!(!plan.include_closed);
    }
}
