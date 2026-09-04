//! Shared types for `cas integrate <platform> <action>`.
//!
//! Platform handlers in [`super::vercel`], [`super::neon`], and [`super::github`]
//! all return [`IntegrationOutcome`] on success so downstream consumers
//! (`cas init`, `cas doctor`, the verify path) can render or react to the
//! result without each platform inventing its own report shape.
//!
//! Errors flow through `anyhow::Result` — see the design note on task cas-e6b6.

use std::path::PathBuf;

/// Which third-party platform an integration targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Vercel,
    Neon,
    Github,
    /// The MechaCassy Slack hub. Unlike the other three this one is a
    /// *machine*-scoped integration: it writes no project SKILL.md and is
    /// therefore absent from [`super::doctor::collect_reports`], which walks
    /// per-project keep blocks. Its doctor row is produced directly by
    /// [`super::mecha_cassy::doctor_row`].
    MechaCassy,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Vercel => "vercel",
            Platform::Neon => "neon",
            Platform::Github => "github",
            Platform::MechaCassy => "mecha-cassy",
        }
    }

    /// Task ID owning the platform-specific implementation.
    /// Used by stub handlers to point users at the correct follow-on task.
    pub fn handler_task(self) -> &'static str {
        match self {
            Platform::Vercel => "cas-8e37",
            Platform::Neon => "cas-1ece",
            Platform::Github => "cas-f425",
            Platform::MechaCassy => "cas-8fad",
        }
    }
}

/// Which action a platform handler is performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationAction {
    /// First-time setup: detect, prompt, write SKILL files.
    Init,
    /// Re-run detection against existing keep-block IDs; update outer content,
    /// preserve user-owned keep blocks (or replace with newly-fetched IDs).
    Refresh,
    /// Read the existing config, ping the platform's MCP, return a structured
    /// staleness report.
    Verify,
}

impl IntegrationAction {
    pub fn as_str(self) -> &'static str {
        match self {
            IntegrationAction::Init => "init",
            IntegrationAction::Refresh => "refresh",
            IntegrationAction::Verify => "verify",
        }
    }
}

/// Status flag for an [`IntegrationOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationStatus {
    /// Init wrote new SKILL files.
    Configured,
    /// Refresh updated outer content; keep blocks preserved.
    Refreshed,
    /// Init found existing populated SKILL files; no changes written.
    AlreadyConfigured,
    /// Verify confirmed the recorded IDs no longer match the platform's
    /// reality (project deleted, branch removed, repo renamed/transferred,
    /// etc.). Distinct from [`IntegrationStatus::TransportError`] — the
    /// platform answered, the answer disagrees.
    Stale,
    /// Verify could not reach the platform's source-of-truth (MCP call
    /// failed, network unavailable, `git` binary missing, auth expired).
    /// Distinct from [`IntegrationStatus::Stale`] — we don't know whether
    /// the recorded IDs are accurate; we couldn't ask. Callers should
    /// surface as a soft warning, not data drift.
    TransportError,
    /// User declined the prompt or platform not detected.
    Skipped,
}

impl IntegrationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            IntegrationStatus::Configured => "configured",
            IntegrationStatus::Refreshed => "refreshed",
            IntegrationStatus::AlreadyConfigured => "already-configured",
            IntegrationStatus::Stale => "stale",
            IntegrationStatus::TransportError => "transport-error",
            IntegrationStatus::Skipped => "skipped",
        }
    }
}

/// Structured result of an integration action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationOutcome {
    pub platform: Platform,
    pub action: IntegrationAction,
    pub status: IntegrationStatus,
    /// Human-readable lines for terminal rendering.
    pub summary: Vec<String>,
    /// Files written or modified during this action (repo-relative).
    pub files: Vec<PathBuf>,
}

impl IntegrationOutcome {
    pub fn new(platform: Platform, action: IntegrationAction, status: IntegrationStatus) -> Self {
        Self {
            platform,
            action,
            status,
            summary: Vec::new(),
            files: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured verify reports (cas-3efe / Phase 3: cas doctor consumes these)
// ---------------------------------------------------------------------------

/// State of a single recorded ID after a verify call.
///
/// `Stale` is reserved for "the platform answered, and the recorded ID is
/// not present" — i.e. genuine drift that the user can fix by running
/// `cas integrate <platform> refresh`. `McpUnreachable` is the transport-
/// error case (MCP server not configured, network down, auth failed); doctor
/// treats this as a *skip*, not a stale, so a missing MCP server doesn't
/// hard-fail `cas doctor` in CI.
///
/// The two-variant split (Stale vs McpUnreachable) is intentionally minimal
/// for cas-3efe; if a future platform needs additional states (e.g.
/// `NotFound` for "ID recorded but never queryable"), add them here at the
/// same time as a `render_one` arm in `cli/integrate/doctor.rs` — adding a
/// variant without wiring the renderer makes the new state silently
/// indistinguishable from `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdState {
    /// ID exists on the platform.
    Ok,
    /// ID is recorded locally but not present on the platform — user should refresh.
    Stale,
    /// MCP/transport call failed; cannot determine state. Carries the error text.
    McpUnreachable(String),
}

impl IdState {
    pub fn as_label(&self) -> &'static str {
        match self {
            IdState::Ok => "ok",
            IdState::Stale => "stale",
            IdState::McpUnreachable(_) => "mcp-unreachable",
        }
    }
}

/// One recorded ID and its verified state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyItem {
    /// Display label (e.g. `"production branchId"`, `"OWNER/REPO"`, `"projectId"`).
    pub label: String,
    /// Raw recorded value as it appears in the keep block.
    pub id: String,
    pub state: IdState,
}

/// Structured verify report consumed by `cas doctor`.
///
/// Distinct from [`IntegrationOutcome`] (which carries the verb-level
/// printed-output story) so doctor can render uniform per-ID rows without
/// re-parsing free-form summary strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub platform: Platform,
    /// `true` when the platform has no SKILL.md (or the file lacks a usable
    /// keep block). Doctor renders this as "not configured" rather than a
    /// warning.
    pub not_configured: bool,
    /// Per-ID rows. Empty when `not_configured == true`.
    pub items: Vec<VerifyItem>,
    /// Free-form notes (e.g. "no `<!-- keep neon-ids -->` block found",
    /// "MCP not configured — set NEON_API_KEY"). Doctor surfaces these
    /// alongside the per-ID rows.
    pub notes: Vec<String>,
}

impl VerifyReport {
    pub fn not_configured(platform: Platform, note: impl Into<String>) -> Self {
        Self {
            platform,
            not_configured: true,
            items: Vec::new(),
            notes: vec![note.into()],
        }
    }

    pub fn ok(platform: Platform) -> Self {
        Self {
            platform,
            not_configured: false,
            items: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// At least one item is in [`IdState::Stale`]. Used by the SessionStart
    /// banner gate in `cli/integrate/doctor::session_start_banner_text`.
    pub fn has_stale(&self) -> bool {
        self.items.iter().any(|i| i.state == IdState::Stale)
    }
}
