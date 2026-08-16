mod loop_tools;
pub(crate) mod verification_tools;
mod worktree_ops;

/// Guidance shared by the public coordination gate and the underlying
/// worktree operation. System A's opt-in configuration is distinct from the
/// factory's own isolation switch, so callers must not be told that one is the
/// gate for the other.
pub(crate) const SYSTEM_A_WORKTREES_DISABLED_MESSAGE: &str = "System A worktrees are disabled by `[worktrees].enabled` in .cas/config.toml.\n\n\
To enable this System-A worktree command, add:\n\n\
[worktrees]\n\
enabled = true\n\n\
Factory isolation worktrees use a separate factory `--worktrees` switch. Existing factory \
worktrees do not enable this System-A command. To create a factory worktree, ask the supervisor \
to run `coordination action=spawn_workers isolate=true` for the task. Use `coordination \
action=worktree_status` to inspect both systems.";
