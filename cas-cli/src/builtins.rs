//! Built-in Cassy content that gets synced to .claude/ or .codex/ directories
//!
//! These definitions are managed by Cassy and regenerated on `cas update`.
//! Files with `managed_by: cas` in frontmatter are overwritten on update.
//! References beneath a managed builtin skill inherit directory ownership and
//! use a last-synced hash to propagate Cassy changes without clobbering local edits.
//!
//! All content uses MCP tools (`mcp__cas__*`).
//!
//! The factory guide skill files are also the source of truth for HooksConfig
//! guidance that gets injected into supervisor/worker context.

use cas_mux::SupervisorCli;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::OnceLock;

/// Factory supervisor guide - embedded at compile time (source of truth)
pub const SUPERVISOR_GUIDE: &str = include_str!("builtins/skills/cas-supervisor.md");

/// Factory worker guide - embedded at compile time (source of truth)
pub const WORKER_GUIDE: &str = include_str!("builtins/skills/cas-worker.md");

/// Shared skills preloaded into factory sessions
pub const TASK_TRACKING_GUIDE: &str = include_str!("builtins/skills/cas-task-tracking.md");
pub const MEMORY_GUIDE: &str = include_str!("builtins/skills/cas-memory-management/SKILL.md");
pub const SEARCH_GUIDE: &str = include_str!("builtins/skills/cas-search.md");
pub const CHECKLIST_GUIDE: &str = include_str!("builtins/skills/cas-supervisor-checklist.md");

/// A built-in file that Cassy manages
#[derive(Clone, Copy)]
pub struct BuiltinFile {
    /// Relative path within .claude/ (e.g., "agents/task-verifier.md")
    pub path: &'static str,
    /// File content (uses MCP tools)
    pub content: &'static str,
}

/// All built-in agents managed by Cassy
pub const BUILTIN_AGENTS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "agents/task-verifier.md",
        content: include_str!("builtins/agents/task-verifier.md"),
    },
    BuiltinFile {
        path: "agents/learning-reviewer.md",
        content: include_str!("builtins/agents/learning-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/rule-reviewer.md",
        content: include_str!("builtins/agents/rule-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/duplicate-detector.md",
        content: include_str!("builtins/agents/duplicate-detector.md"),
    },
    BuiltinFile {
        path: "agents/session-summarizer.md",
        content: include_str!("builtins/agents/session-summarizer.md"),
    },
    // DEPRECATED (Phase 1 subsystem A, EPIC cas-0750): the legacy
    // `code-reviewer` agent is replaced by the `cas-code-review` multi-persona
    // skill. The entry is kept in BUILTIN_AGENTS only so `cas sync` overwrites
    // any downstream `.claude/agents/code-reviewer.md` with the deprecation
    // stub checked into the repo. Remove after downstream caches expire.
    BuiltinFile {
        path: "agents/code-reviewer.md",
        content: include_str!("builtins/agents/code-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/git-history-analyzer.md",
        content: include_str!("builtins/agents/git-history-analyzer.md"),
    },
    BuiltinFile {
        path: "agents/issue-intelligence-analyst.md",
        content: include_str!("builtins/agents/issue-intelligence-analyst.md"),
    },
];

/// All built-in agents managed by Cassy for Codex
pub const CODEX_BUILTIN_AGENTS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "agents/task-verifier.md",
        content: include_str!("builtins/codex/agents/task-verifier.md"),
    },
    BuiltinFile {
        path: "agents/learning-reviewer.md",
        content: include_str!("builtins/codex/agents/learning-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/rule-reviewer.md",
        content: include_str!("builtins/codex/agents/rule-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/duplicate-detector.md",
        content: include_str!("builtins/codex/agents/duplicate-detector.md"),
    },
    BuiltinFile {
        path: "agents/session-summarizer.md",
        content: include_str!("builtins/codex/agents/session-summarizer.md"),
    },
    // DEPRECATED (Phase 1 subsystem A, EPIC cas-0750): see the note on the
    // claude-mirror entry above. Kept only so `cas sync` overwrites stale
    // downstream copies with the deprecation stub.
    BuiltinFile {
        path: "agents/code-reviewer.md",
        content: include_str!("builtins/codex/agents/code-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/factory-supervisor.md",
        content: include_str!("builtins/codex/agents/factory-supervisor.md"),
    },
    BuiltinFile {
        path: "agents/git-history-analyzer.md",
        content: include_str!("builtins/codex/agents/git-history-analyzer.md"),
    },
    BuiltinFile {
        path: "agents/issue-intelligence-analyst.md",
        content: include_str!("builtins/codex/agents/issue-intelligence-analyst.md"),
    },
];

/// All built-in skills managed by Cassy
pub const BUILTIN_SKILLS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "skills/cas-memory-management/SKILL.md",
        content: include_str!("builtins/skills/cas-memory-management/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/schema.yaml",
        content: include_str!("builtins/skills/cas-memory-management/references/schema.yaml"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/body-templates.md",
        content: include_str!("builtins/skills/cas-memory-management/references/body-templates.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/overlap-detection.md",
        content: include_str!(
            "builtins/skills/cas-memory-management/references/overlap-detection.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/lifecycle-and-storage.md",
        content: include_str!(
            "builtins/skills/cas-memory-management/references/lifecycle-and-storage.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-search/SKILL.md",
        content: include_str!("builtins/skills/cas-search.md"),
    },
    BuiltinFile {
        path: "skills/cas-task-tracking/SKILL.md",
        content: include_str!("builtins/skills/cas-task-tracking.md"),
    },
    // session-learn (cas-39f5, EPIC cas-ebea): 7-signal session classifier
    // borrowed from third-brain-v5-skills. The skill body is also the
    // runtime prompt template embedded by the Stop hook handler (decision:
    // in-process for v1, see the skill body's "in-process vs subprocess"
    // section). v1 default: `[memory] session_learn_auto = false` —
    // manual-invocation only until user opts in.
    BuiltinFile {
        path: "skills/session-learn/SKILL.md",
        content: include_str!("builtins/skills/session-learn/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/SKILL.md",
        content: include_str!("builtins/skills/cas-supervisor.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/preflight.md",
        content: include_str!("builtins/skills/cas-supervisor/references/preflight.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/intake.md",
        content: include_str!("builtins/skills/cas-supervisor/references/intake.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/planning.md",
        content: include_str!("builtins/skills/cas-supervisor/references/planning.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/workflow.md",
        content: include_str!("builtins/skills/cas-supervisor/references/workflow.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/worker-recovery.md",
        content: include_str!("builtins/skills/cas-supervisor/references/worker-recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reference.md",
        content: include_str!("builtins/skills/cas-supervisor/references/reference.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/code-review-queue.md",
        content: include_str!("builtins/skills/cas-supervisor/references/code-review-queue.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/filing-cas-bugs.md",
        content: include_str!("builtins/skills/cas-supervisor/references/filing-cas-bugs.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/model-selection.md",
        content: include_str!("builtins/skills/cas-supervisor/references/model-selection.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reminders.md",
        content: include_str!("builtins/skills/cas-supervisor/references/reminders.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor-checklist/SKILL.md",
        content: include_str!("builtins/skills/cas-supervisor-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/SKILL.md",
        content: include_str!("builtins/skills/cas-worker.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/close-gate.md",
        content: include_str!("builtins/skills/cas-worker/references/close-gate.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/recovery.md",
        content: include_str!("builtins/skills/cas-worker/references/recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/details.md",
        content: include_str!("builtins/skills/cas-worker/references/details.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/discipline.md",
        content: include_str!("builtins/skills/cas-worker/references/discipline.md"),
    },
    // verify-before-claim skill (cas-5b2a, EPIC cas-ebea third-brain borrow).
    // Pre-close agent-discipline layer that forces workers to name, run, and
    // capture a proof command before claiming done. Advisory in v1; the
    // verification_store + close-gate.md self-checks remain the mechanical
    // gate underneath.
    BuiltinFile {
        path: "skills/verify-before-claim/SKILL.md",
        content: include_str!("builtins/skills/verify-before-claim/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-codex-exec/SKILL.md",
        content: include_str!("builtins/skills/cas-codex-exec/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/SKILL.md",
        content: include_str!("builtins/skills/cli-routing/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/references/routing.md",
        content: include_str!("builtins/skills/cli-routing/references/routing.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/SKILL.md",
        content: include_str!("builtins/skills/cas-brainstorm/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/handoff.md",
        content: include_str!("builtins/skills/cas-brainstorm/references/handoff.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/requirements-capture.md",
        content: include_str!("builtins/skills/cas-brainstorm/references/requirements-capture.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/SKILL.md",
        content: include_str!("builtins/skills/cas-ideate/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/references/post-ideation-workflow.md",
        content: include_str!("builtins/skills/cas-ideate/references/post-ideation-workflow.md"),
    },
    // cas-code-review (Phase 1 subsystem A, EPIC cas-0750).
    // Multi-persona code-review skill that replaces the legacy `code-reviewer`
    // agent. The old agent entry below is kept only to propagate a deprecation
    // stub via `cas sync`; all real functionality lives in this skill.
    BuiltinFile {
        path: "skills/cas-code-review/SKILL.md",
        content: include_str!("builtins/skills/cas-code-review/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/findings-schema.md",
        content: include_str!("builtins/skills/cas-code-review/references/findings-schema.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/correctness.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/correctness.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/testing.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/testing.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/maintainability.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/maintainability.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/project-standards.md",
        content: include_str!(
            "builtins/skills/cas-code-review/references/personas/project-standards.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/security.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/security.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/performance.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/performance.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/adversarial.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/adversarial.md"),
    },
    // fallow persona — deterministic CLI adapter routed through the shared
    // Codex reviewer transport. It translates `fallow audit` findings into
    // ReviewerOutput and self-skips on non-JS/TS repos / diffs.
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/fallow.md",
        content: include_str!("builtins/skills/cas-code-review/references/personas/fallow.md"),
    },
    // project-overview skill (EPIC cas-19a2b): generates
    // docs/PRODUCT_OVERVIEW.md for any project and writes a thin memory
    // pointer so Cassy search surfaces the doc.
    BuiltinFile {
        path: "skills/project-overview/SKILL.md",
        content: include_str!("builtins/skills/project-overview/SKILL.md"),
    },
    // codemap skill (cas-4d84): remediation skill for the codemap
    // freshness gate. Generates .claude/CODEMAP.md so SessionStart and
    // PreToolUse stop nagging.
    BuiltinFile {
        path: "skills/codemap/SKILL.md",
        content: include_str!("builtins/skills/codemap/SKILL.md"),
    },
    // cas-servers skill (cas-7c93, GH #87): the sanctioned lifecycle for
    // long-running servers. Registered servers are the only ones that survive
    // worker containment teardown, so the guidance has to reach agents before
    // they reach for `npm run dev &`.
    BuiltinFile {
        path: "skills/cas-servers/SKILL.md",
        content: include_str!("builtins/skills/cas-servers/SKILL.md"),
    },
    // cas-1219: MCP installation and diagnosis guidance from the field-tested
    // operator runbook. The linked diagnosis reference ships with it.
    BuiltinFile {
        path: "skills/mcp-integration/SKILL.md",
        content: include_str!("builtins/skills/mcp-integration/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/mcp-integration/references/diagnosis.md",
        content: include_str!("builtins/skills/mcp-integration/references/diagnosis.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/SKILL.md",
        content: include_str!("builtins/skills/cas-viktor/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/references/gateway.md",
        content: include_str!("builtins/skills/cas-viktor/references/gateway.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/SKILL.md",
        content: include_str!("builtins/skills/cas-html-reports/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/report-types.md",
        content: include_str!("builtins/skills/cas-html-reports/references/report-types.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/presentation-rules.md",
        content: include_str!("builtins/skills/cas-html-reports/references/presentation-rules.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/technical-contract.md",
        content: include_str!("builtins/skills/cas-html-reports/references/technical-contract.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/review-checklist.md",
        content: include_str!("builtins/skills/cas-html-reports/references/review-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/sources.md",
        content: include_str!("builtins/skills/cas-html-reports/references/sources.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/engineering-investigation.html",
        content: include_str!("builtins/skills/cas-html-reports/references/examples/engineering-investigation.html"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/financial-quarterly-brief.html",
        content: include_str!("builtins/skills/cas-html-reports/references/examples/financial-quarterly-brief.html"),
    },
    // cas-1e7e: cross-harness data visualization guidance for static evidence artifacts.
    BuiltinFile {
        path: "skills/cas-dataviz/SKILL.md",
        content: include_str!("builtins/skills/cas-dataviz/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/design-review.md",
        content: include_str!("builtins/skills/cas-dataviz/references/design-review.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/quality-checklist.md",
        content: include_str!("builtins/skills/cas-dataviz/references/quality-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/scripts/validate_palette.js",
        content: include_str!("builtins/skills/cas-dataviz/scripts/validate_palette.js"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/examples/2026-08-11-commit-classes.html",
        content: include_str!("builtins/skills/cas-dataviz/examples/2026-08-11-commit-classes.html"),
    },
    // design-spec skill (GH #64): generates DESIGN.md — the UI/UX source of
    // truth (normative token frontmatter + 8 sections). Design counterpart to
    // codemap (structure) and project-overview (domain).
    BuiltinFile {
        path: "skills/design-spec/SKILL.md",
        content: include_str!("builtins/skills/design-spec/SKILL.md"),
    },
    // release-notes skill (GH #65): drafts/posts the user + dev Slack threads
    // for every staging/main merge and installs the canonical rubric template
    // at docs/release-notes/RUBRIC.md when a project has none.
    BuiltinFile {
        path: "skills/release-notes/SKILL.md",
        content: include_str!("builtins/skills/release-notes/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/release-notes/references/RUBRIC-template.md",
        content: include_str!("builtins/skills/release-notes/references/RUBRIC-template.md"),
    },
    // cas-github-issues skill (cas-ff2f, GH #94): the recurring GitHub Issues
    // sweep — dedupe double-filings, verify-and-close fixed claims, task new
    // issues into the active epic (successor epic when none is open), unblock
    // chained tasks whose lane merged, and file observed defects. The hourly
    // cron prompt invokes this skill by name, so it must resolve after sync.
    BuiltinFile {
        path: "skills/cas-github-issues/SKILL.md",
        content: include_str!("builtins/skills/cas-github-issues/SKILL.md"),
    },
    // cas-nuxt-playwright skill: unified Nuxt 3 + Playwright E2E testing
    // guide. Replaces the legacy user-level cas-playwright-debug skill with
    // a single builtin that covers both writing and debugging tests. Modeled
    // after the gabber-studio production test suite; Firebase-focused.
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/SKILL.md",
        content: include_str!("builtins/skills/cas-nuxt-playwright/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/references/auth-fixture-template.md",
        content: include_str!(
            "builtins/skills/cas-nuxt-playwright/references/auth-fixture-template.md"
        ),
    },
    // fallow skill: vendored from https://github.com/fallow-rs/fallow-skills
    // (MIT, Bart Waardenburg). Codebase intelligence for JS/TS — dead code,
    // duplication, complexity, boundaries, feature flags. SKILL.md +
    // 3 references match the upstream layout; only `managed_by: cas` is
    // injected so `cas sync` keeps user copies fresh.
    BuiltinFile {
        path: "skills/fallow/SKILL.md",
        content: include_str!("builtins/skills/fallow/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/cli-reference.md",
        content: include_str!("builtins/skills/fallow/references/cli-reference.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/gotchas.md",
        content: include_str!("builtins/skills/fallow/references/gotchas.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/patterns.md",
        content: include_str!("builtins/skills/fallow/references/patterns.md"),
    },
    // cas-writing-for-agents: adapted from mattpocock/skills (MIT, © 2026 Matt Pocock).
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL.md",
        content: include_str!("builtins/skills/cas-writing-for-agents/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL-MECHANICS.md",
        content: include_str!("builtins/skills/cas-writing-for-agents/SKILL-MECHANICS.md"),
    },
    BuiltinFile {
        path: "skills/cas-diagnosing-bugs/SKILL.md",
        content: include_str!("builtins/skills/cas-diagnosing-bugs/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-domain-modeling/SKILL.md",
        content: include_str!("builtins/skills/cas-domain-modeling/SKILL.md"),
    },
    BuiltinFile { path: "skills/cas-codebase-design/SKILL.md", content: include_str!("builtins/skills/cas-codebase-design/SKILL.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DEEPENING.md", content: include_str!("builtins/skills/cas-codebase-design/DEEPENING.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DESIGN-IT-TWICE.md", content: include_str!("builtins/skills/cas-codebase-design/DESIGN-IT-TWICE.md") },
    BuiltinFile { path: "skills/cas-tdd/SKILL.md", content: include_str!("builtins/skills/cas-tdd/SKILL.md") },
    BuiltinFile { path: "skills/cas-tdd/tests.md", content: include_str!("builtins/skills/cas-tdd/tests.md") },
    BuiltinFile { path: "skills/cas-tdd/mocking.md", content: include_str!("builtins/skills/cas-tdd/mocking.md") },
    BuiltinFile { path: "skills/cas-wizard/SKILL.md", content: include_str!("builtins/skills/cas-wizard/SKILL.md") },
    BuiltinFile { path: "skills/cas-wizard/template.sh", content: include_str!("builtins/skills/cas-wizard/template.sh") },
    BuiltinFile { path: "skills/cas-resolving-merge-conflicts/SKILL.md", content: include_str!("builtins/skills/cas-resolving-merge-conflicts/SKILL.md") },
    BuiltinFile { path: "skills/cas-to-questionnaire/SKILL.md", content: include_str!("builtins/skills/cas-to-questionnaire/SKILL.md") },
];

/// Built-in Workflow scripts shipped to `.claude/workflows/` on `cas update --sync`.
///
/// Workflow scripts are machine-generated JS files with no user-customizable
/// frontmatter. Unlike skills/agents (which use the `managed_by: cas` gate),
/// workflows are always force-written on sync — they are pure Cassy-managed
/// artifacts and should never be hand-edited by users. The `sync_workflows`
/// function handles this unconditional write.
///
/// Only Claude-harness workflows are shipped here. Codex does not use the
/// Claude Code Workflow tool.
pub const BUILTIN_WORKFLOWS: &[BuiltinFile] = &[
    // cas-code-review Steps 3-4: parallel persona dispatch + deterministic merge
    // (Phase B of EPIC cas-b667). Invoked by the cas-code-review skill wrapper.
    BuiltinFile {
        path: "workflows/cas-code-review.js",
        content: include_str!("builtins/workflows/cas-code-review.js"),
    },
];

/// All built-in skills managed by Cassy for Codex
pub const CODEX_BUILTIN_SKILLS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "skills/cas-memory-management/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-memory-management/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/schema.yaml",
        content: include_str!("builtins/codex/skills/cas-memory-management/references/schema.yaml"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/body-templates.md",
        content: include_str!(
            "builtins/codex/skills/cas-memory-management/references/body-templates.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/overlap-detection.md",
        content: include_str!(
            "builtins/codex/skills/cas-memory-management/references/overlap-detection.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/lifecycle-and-storage.md",
        content: include_str!(
            "builtins/codex/skills/cas-memory-management/references/lifecycle-and-storage.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-search/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-search.md"),
    },
    BuiltinFile {
        path: "skills/cas-task-tracking/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-task-tracking.md"),
    },
    // session-learn (cas-39f5, EPIC cas-ebea) — Codex mirror. Kept
    // byte-identical to the .claude copy by regression test in
    // `test_session_learn_mirrors_are_identical`.
    BuiltinFile {
        path: "skills/session-learn/SKILL.md",
        content: include_str!("builtins/codex/skills/session-learn/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-supervisor.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/preflight.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/preflight.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/intake.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/intake.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/planning.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/planning.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/workflow.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/workflow.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/worker-recovery.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/worker-recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reference.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/reference.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/code-review-queue.md",
        content: include_str!(
            "builtins/codex/skills/cas-supervisor/references/code-review-queue.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/filing-cas-bugs.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/filing-cas-bugs.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/model-selection.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/model-selection.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reminders.md",
        content: include_str!("builtins/codex/skills/cas-supervisor/references/reminders.md"),
    },
    BuiltinFile {
        path: "skills/cas-codex-supervisor-checklist/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-codex-supervisor-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-worker.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/close-gate.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/close-gate.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/recovery.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/details.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/details.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/discipline.md",
        content: include_str!("builtins/codex/skills/cas-worker/references/discipline.md"),
    },
    // verify-before-claim skill (cas-5b2a) — codex mirror. See claude-side
    // entry above for context.
    BuiltinFile {
        path: "skills/verify-before-claim/SKILL.md",
        content: include_str!("builtins/codex/skills/verify-before-claim/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-codex-exec/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-codex-exec/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/SKILL.md",
        content: include_str!("builtins/codex/skills/cli-routing/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/references/routing.md",
        content: include_str!("builtins/codex/skills/cli-routing/references/routing.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-brainstorm/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/handoff.md",
        content: include_str!("builtins/codex/skills/cas-brainstorm/references/handoff.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/requirements-capture.md",
        content: include_str!(
            "builtins/codex/skills/cas-brainstorm/references/requirements-capture.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-ideate/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-ideate/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/references/post-ideation-workflow.md",
        content: include_str!(
            "builtins/codex/skills/cas-ideate/references/post-ideation-workflow.md"
        ),
    },
    // cas-code-review (Phase 1 subsystem A, EPIC cas-0750) — codex mirror.
    BuiltinFile {
        path: "skills/cas-code-review/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-code-review/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/findings-schema.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/findings-schema.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/correctness.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/correctness.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/testing.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/testing.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/maintainability.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/maintainability.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/project-standards.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/project-standards.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/security.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/security.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/performance.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/performance.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/adversarial.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/adversarial.md"
        ),
    },
    // fallow persona — codex mirror. See claude-side entry above.
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/fallow.md",
        content: include_str!(
            "builtins/codex/skills/cas-code-review/references/personas/fallow.md"
        ),
    },
    // project-overview skill (EPIC cas-19a2b) — codex mirror.
    BuiltinFile {
        path: "skills/project-overview/SKILL.md",
        content: include_str!("builtins/codex/skills/project-overview/SKILL.md"),
    },
    // codemap skill (cas-4d84) — codex mirror.
    BuiltinFile {
        path: "skills/codemap/SKILL.md",
        content: include_str!("builtins/codex/skills/codemap/SKILL.md"),
    },
    // cas-servers skill (cas-7c93, GH #87) — codex mirror. Kept byte-identical
    // to the .claude copy by `test_builtin_skills_contains_cas_servers`.
    BuiltinFile {
        path: "skills/cas-servers/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-servers/SKILL.md"),
    },
    // cas-1219: byte-identical Codex mirror of the field-tested MCP guidance.
    BuiltinFile {
        path: "skills/mcp-integration/SKILL.md",
        content: include_str!("builtins/codex/skills/mcp-integration/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/mcp-integration/references/diagnosis.md",
        content: include_str!("builtins/codex/skills/mcp-integration/references/diagnosis.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-viktor/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/references/gateway.md",
        content: include_str!("builtins/codex/skills/cas-viktor/references/gateway.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/report-types.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/report-types.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/presentation-rules.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/presentation-rules.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/technical-contract.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/technical-contract.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/review-checklist.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/review-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/sources.md",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/sources.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/engineering-investigation.html",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/examples/engineering-investigation.html"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/financial-quarterly-brief.html",
        content: include_str!("builtins/codex/skills/cas-html-reports/references/examples/financial-quarterly-brief.html"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-dataviz/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/design-review.md",
        content: include_str!("builtins/codex/skills/cas-dataviz/references/design-review.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/quality-checklist.md",
        content: include_str!("builtins/codex/skills/cas-dataviz/references/quality-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/scripts/validate_palette.js",
        content: include_str!("builtins/codex/skills/cas-dataviz/scripts/validate_palette.js"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/examples/2026-08-11-commit-classes.html",
        content: include_str!("builtins/codex/skills/cas-dataviz/examples/2026-08-11-commit-classes.html"),
    },
    // design-spec skill (GH #64) — codex mirror.
    BuiltinFile {
        path: "skills/design-spec/SKILL.md",
        content: include_str!("builtins/codex/skills/design-spec/SKILL.md"),
    },
    // release-notes skill (GH #65) — codex mirror.
    BuiltinFile {
        path: "skills/release-notes/SKILL.md",
        content: include_str!("builtins/codex/skills/release-notes/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/release-notes/references/RUBRIC-template.md",
        content: include_str!("builtins/codex/skills/release-notes/references/RUBRIC-template.md"),
    },
    // cas-github-issues skill (cas-ff2f, GH #94) — codex mirror. Byte-identical
    // to the claude copy except for the harness tool prefix.
    BuiltinFile {
        path: "skills/cas-github-issues/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-github-issues/SKILL.md"),
    },
    // cas-nuxt-playwright skill — codex mirror.
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-nuxt-playwright/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/references/auth-fixture-template.md",
        content: include_str!(
            "builtins/codex/skills/cas-nuxt-playwright/references/auth-fixture-template.md"
        ),
    },
    // fallow skill — codex mirror. See the claude-side entry above for the
    // upstream attribution (fallow-rs/fallow-skills, MIT).
    BuiltinFile {
        path: "skills/fallow/SKILL.md",
        content: include_str!("builtins/codex/skills/fallow/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/cli-reference.md",
        content: include_str!("builtins/codex/skills/fallow/references/cli-reference.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/gotchas.md",
        content: include_str!("builtins/codex/skills/fallow/references/gotchas.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/patterns.md",
        content: include_str!("builtins/codex/skills/fallow/references/patterns.md"),
    },
    // cas-writing-for-agents: Codex mirror of the MIT Matt Pocock import above.
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-writing-for-agents/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL-MECHANICS.md",
        content: include_str!("builtins/codex/skills/cas-writing-for-agents/SKILL-MECHANICS.md"),
    },
    BuiltinFile {
        path: "skills/cas-diagnosing-bugs/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-diagnosing-bugs/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-domain-modeling/SKILL.md",
        content: include_str!("builtins/codex/skills/cas-domain-modeling/SKILL.md"),
    },
    BuiltinFile { path: "skills/cas-codebase-design/SKILL.md", content: include_str!("builtins/codex/skills/cas-codebase-design/SKILL.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DEEPENING.md", content: include_str!("builtins/codex/skills/cas-codebase-design/DEEPENING.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DESIGN-IT-TWICE.md", content: include_str!("builtins/codex/skills/cas-codebase-design/DESIGN-IT-TWICE.md") },
    BuiltinFile { path: "skills/cas-tdd/SKILL.md", content: include_str!("builtins/codex/skills/cas-tdd/SKILL.md") },
    BuiltinFile { path: "skills/cas-tdd/tests.md", content: include_str!("builtins/codex/skills/cas-tdd/tests.md") },
    BuiltinFile { path: "skills/cas-tdd/mocking.md", content: include_str!("builtins/codex/skills/cas-tdd/mocking.md") },
    BuiltinFile { path: "skills/cas-wizard/SKILL.md", content: include_str!("builtins/codex/skills/cas-wizard/SKILL.md") },
    BuiltinFile { path: "skills/cas-wizard/template.sh", content: include_str!("builtins/codex/skills/cas-wizard/template.sh") },
    BuiltinFile { path: "skills/cas-resolving-merge-conflicts/SKILL.md", content: include_str!("builtins/codex/skills/cas-resolving-merge-conflicts/SKILL.md") },
    BuiltinFile { path: "skills/cas-to-questionnaire/SKILL.md", content: include_str!("builtins/codex/skills/cas-to-questionnaire/SKILL.md") },
];

/// All built-in agents managed by Cassy for Grok (EPIC cas-8888, Phase 5 /
/// cas-6f46). Derived from the Claude set (`BUILTIN_AGENTS`), not the Codex
/// one: Grok's capability tier matches Claude's (hooks + subagents +
/// textbox-submit all supported), so the Claude agent prompts are the
/// correct behavioral starting point. Only the tool prefix changes
/// (`mcp__cas__` → `cas__` — Grok namespaces MCP tools as `<server>__<tool>`
/// via its own search_tool/use_tool dispatch, with no `mcp__` wrapper).
pub const GROK_BUILTIN_AGENTS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "agents/task-verifier.md",
        content: include_str!("builtins/grok/agents/task-verifier.md"),
    },
    BuiltinFile {
        path: "agents/learning-reviewer.md",
        content: include_str!("builtins/grok/agents/learning-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/rule-reviewer.md",
        content: include_str!("builtins/grok/agents/rule-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/duplicate-detector.md",
        content: include_str!("builtins/grok/agents/duplicate-detector.md"),
    },
    BuiltinFile {
        path: "agents/session-summarizer.md",
        content: include_str!("builtins/grok/agents/session-summarizer.md"),
    },
    // DEPRECATED (Phase 1 subsystem A, EPIC cas-0750): see the note on the
    // claude-mirror entry in BUILTIN_AGENTS. Kept only so `cas sync`
    // overwrites stale downstream copies with the deprecation stub.
    BuiltinFile {
        path: "agents/code-reviewer.md",
        content: include_str!("builtins/grok/agents/code-reviewer.md"),
    },
    BuiltinFile {
        path: "agents/git-history-analyzer.md",
        content: include_str!("builtins/grok/agents/git-history-analyzer.md"),
    },
    BuiltinFile {
        path: "agents/issue-intelligence-analyst.md",
        content: include_str!("builtins/grok/agents/issue-intelligence-analyst.md"),
    },
];

/// All built-in skills managed by Cassy for Grok (EPIC cas-8888, Phase 5 /
/// cas-6f46; required-capability parity closed by cas-cc8c, full general-skill
/// parity closed by cas-20f2).
///
/// Covers every factory-critical required capability (see
/// `REQUIRED_FACTORY_CAPABILITIES`) AND every general Cassy skill Claude/Codex
/// expose (see `GENERAL_PARITY_CAPABILITIES`: session-learn, codemap,
/// project-overview, fallow, cas-nuxt-playwright, cas-codex-exec) in its OWN
/// right. A Grok session MUST NOT depend on implicitly inheriting `~/.claude`
/// for ANY skill: the factory can run against a project-local `.grok` mirror or
/// a `~/.grok` home with no Claude tree present, so every twin is installed here
/// directly. The only intentional differences from the Claude set are the tool
/// prefix and the supervisor-checklist twin spelling — no capability is dropped.
///
/// Tool-prefix content is modeled on the Claude originals (matching
/// capability tier — hooks/subagents/textbox-submit all supported, unlike
/// Codex) with `mcp__cas__` swapped for `cas__`. The supervisor checklist
/// specifically is built from the Claude version (not Codex's "no hooks"
/// compensation variant), since Grok has real SessionStart hooks like
/// Claude does.
pub const GROK_BUILTIN_SKILLS: &[BuiltinFile] = &[
    BuiltinFile {
        path: "skills/cas-worker/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-worker.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/close-gate.md",
        content: include_str!("builtins/grok/skills/cas-worker/references/close-gate.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/recovery.md",
        content: include_str!("builtins/grok/skills/cas-worker/references/recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/details.md",
        content: include_str!("builtins/grok/skills/cas-worker/references/details.md"),
    },
    BuiltinFile {
        path: "skills/cas-worker/references/discipline.md",
        content: include_str!("builtins/grok/skills/cas-worker/references/discipline.md"),
    },
    BuiltinFile {
        path: "skills/verify-before-claim/SKILL.md",
        content: include_str!("builtins/grok/skills/verify-before-claim/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-supervisor.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/preflight.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/preflight.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/intake.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/intake.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/planning.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/planning.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/workflow.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/workflow.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/model-selection.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/model-selection.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reminders.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/reminders.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/worker-recovery.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/worker-recovery.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/reference.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/reference.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/code-review-queue.md",
        content: include_str!(
            "builtins/grok/skills/cas-supervisor/references/code-review-queue.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-supervisor/references/filing-cas-bugs.md",
        content: include_str!("builtins/grok/skills/cas-supervisor/references/filing-cas-bugs.md"),
    },
    BuiltinFile {
        path: "skills/cas-supervisor-checklist/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-supervisor-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-task-tracking/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-task-tracking.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-memory-management/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/schema.yaml",
        content: include_str!("builtins/grok/skills/cas-memory-management/references/schema.yaml"),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/body-templates.md",
        content: include_str!(
            "builtins/grok/skills/cas-memory-management/references/body-templates.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/overlap-detection.md",
        content: include_str!(
            "builtins/grok/skills/cas-memory-management/references/overlap-detection.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-memory-management/references/lifecycle-and-storage.md",
        content: include_str!(
            "builtins/grok/skills/cas-memory-management/references/lifecycle-and-storage.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-code-review/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/findings-schema.md",
        content: include_str!("builtins/grok/skills/cas-code-review/references/findings-schema.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/fixer.md",
        content: include_str!("builtins/grok/skills/cas-code-review/references/fixer.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/adversarial.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/adversarial.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/correctness.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/correctness.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/fallow.md",
        content: include_str!("builtins/grok/skills/cas-code-review/references/personas/fallow.md"),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/maintainability.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/maintainability.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/performance.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/performance.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/project-standards.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/project-standards.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/security.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/security.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-code-review/references/personas/testing.md",
        content: include_str!(
            "builtins/grok/skills/cas-code-review/references/personas/testing.md"
        ),
    },
    // cas-cc8c: required-capability parity — a Grok factory session must resolve
    // the search and brainstorm/ideation capabilities from its OWN catalog, not
    // by implicitly inheriting `~/.claude` (which the factory can no longer rely
    // on). Twins are the Claude sources with the `mcp__cas__` → `cas__` prefix
    // swap (Grok's capability tier matches Claude's), same as the other Grok
    // skills above.
    BuiltinFile {
        path: "skills/cas-search/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-search.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-brainstorm/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/handoff.md",
        content: include_str!("builtins/grok/skills/cas-brainstorm/references/handoff.md"),
    },
    BuiltinFile {
        path: "skills/cas-brainstorm/references/requirements-capture.md",
        content: include_str!(
            "builtins/grok/skills/cas-brainstorm/references/requirements-capture.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-ideate/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-ideate/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-ideate/references/post-ideation-workflow.md",
        content: include_str!(
            "builtins/grok/skills/cas-ideate/references/post-ideation-workflow.md"
        ),
    },
    // cas-20f2: full GENERAL-skill parity — Grok now owns twins for every
    // general Cassy skill Claude/Codex expose (session-learn, codemap,
    // project-overview, fallow, cas-nuxt-playwright, cas-codex-exec), so a Grok
    // session never has to fall back to `~/.claude`. Twins are the Claude
    // sources with the `mcp__cas__` → `cas__` swap; fallow/cas-nuxt-playwright/
    // cas-codex-exec make no CAS MCP calls so their twins are byte-identical.
    BuiltinFile {
        path: "skills/session-learn/SKILL.md",
        content: include_str!("builtins/grok/skills/session-learn/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/codemap/SKILL.md",
        content: include_str!("builtins/grok/skills/codemap/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/project-overview/SKILL.md",
        content: include_str!("builtins/grok/skills/project-overview/SKILL.md"),
    },
    // cas-servers skill (cas-7c93, GH #87) — grok twin.
    BuiltinFile {
        path: "skills/cas-servers/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-servers/SKILL.md"),
    },
    // cas-1219: byte-identical Grok mirror of the field-tested MCP guidance.
    BuiltinFile {
        path: "skills/mcp-integration/SKILL.md",
        content: include_str!("builtins/grok/skills/mcp-integration/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/mcp-integration/references/diagnosis.md",
        content: include_str!("builtins/grok/skills/mcp-integration/references/diagnosis.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-viktor/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-viktor/references/gateway.md",
        content: include_str!("builtins/grok/skills/cas-viktor/references/gateway.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/report-types.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/report-types.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/presentation-rules.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/presentation-rules.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/technical-contract.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/technical-contract.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/review-checklist.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/review-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/sources.md",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/sources.md"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/engineering-investigation.html",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/examples/engineering-investigation.html"),
    },
    BuiltinFile {
        path: "skills/cas-html-reports/references/examples/financial-quarterly-brief.html",
        content: include_str!("builtins/grok/skills/cas-html-reports/references/examples/financial-quarterly-brief.html"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-dataviz/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/design-review.md",
        content: include_str!("builtins/grok/skills/cas-dataviz/references/design-review.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/references/quality-checklist.md",
        content: include_str!("builtins/grok/skills/cas-dataviz/references/quality-checklist.md"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/scripts/validate_palette.js",
        content: include_str!("builtins/grok/skills/cas-dataviz/scripts/validate_palette.js"),
    },
    BuiltinFile {
        path: "skills/cas-dataviz/examples/2026-08-11-commit-classes.html",
        content: include_str!("builtins/grok/skills/cas-dataviz/examples/2026-08-11-commit-classes.html"),
    },
    // design-spec skill (GH #64) — grok twin.
    BuiltinFile {
        path: "skills/design-spec/SKILL.md",
        content: include_str!("builtins/grok/skills/design-spec/SKILL.md"),
    },
    // release-notes skill (GH #65) — grok twin.
    BuiltinFile {
        path: "skills/release-notes/SKILL.md",
        content: include_str!("builtins/grok/skills/release-notes/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/release-notes/references/RUBRIC-template.md",
        content: include_str!("builtins/grok/skills/release-notes/references/RUBRIC-template.md"),
    },
    BuiltinFile {
        path: "skills/fallow/SKILL.md",
        content: include_str!("builtins/grok/skills/fallow/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/cli-reference.md",
        content: include_str!("builtins/grok/skills/fallow/references/cli-reference.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/gotchas.md",
        content: include_str!("builtins/grok/skills/fallow/references/gotchas.md"),
    },
    BuiltinFile {
        path: "skills/fallow/references/patterns.md",
        content: include_str!("builtins/grok/skills/fallow/references/patterns.md"),
    },
    // cas-github-issues skill (cas-ff2f, GH #94) — grok twin.
    BuiltinFile {
        path: "skills/cas-github-issues/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-github-issues/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-nuxt-playwright/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-nuxt-playwright/references/auth-fixture-template.md",
        content: include_str!(
            "builtins/grok/skills/cas-nuxt-playwright/references/auth-fixture-template.md"
        ),
    },
    BuiltinFile {
        path: "skills/cas-codex-exec/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-codex-exec/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/SKILL.md",
        content: include_str!("builtins/grok/skills/cli-routing/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cli-routing/references/routing.md",
        content: include_str!("builtins/grok/skills/cli-routing/references/routing.md"),
    },
    // cas-writing-for-agents: Grok mirror of the MIT Matt Pocock import above.
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-writing-for-agents/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-writing-for-agents/SKILL-MECHANICS.md",
        content: include_str!("builtins/grok/skills/cas-writing-for-agents/SKILL-MECHANICS.md"),
    },
    BuiltinFile {
        path: "skills/cas-diagnosing-bugs/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-diagnosing-bugs/SKILL.md"),
    },
    BuiltinFile {
        path: "skills/cas-domain-modeling/SKILL.md",
        content: include_str!("builtins/grok/skills/cas-domain-modeling/SKILL.md"),
    },
    BuiltinFile { path: "skills/cas-codebase-design/SKILL.md", content: include_str!("builtins/grok/skills/cas-codebase-design/SKILL.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DEEPENING.md", content: include_str!("builtins/grok/skills/cas-codebase-design/DEEPENING.md") },
    BuiltinFile { path: "skills/cas-codebase-design/DESIGN-IT-TWICE.md", content: include_str!("builtins/grok/skills/cas-codebase-design/DESIGN-IT-TWICE.md") },
    BuiltinFile { path: "skills/cas-tdd/SKILL.md", content: include_str!("builtins/grok/skills/cas-tdd/SKILL.md") },
    BuiltinFile { path: "skills/cas-tdd/tests.md", content: include_str!("builtins/grok/skills/cas-tdd/tests.md") },
    BuiltinFile { path: "skills/cas-tdd/mocking.md", content: include_str!("builtins/grok/skills/cas-tdd/mocking.md") },
    BuiltinFile { path: "skills/cas-wizard/SKILL.md", content: include_str!("builtins/grok/skills/cas-wizard/SKILL.md") },
    BuiltinFile { path: "skills/cas-wizard/template.sh", content: include_str!("builtins/grok/skills/cas-wizard/template.sh") },
    BuiltinFile { path: "skills/cas-resolving-merge-conflicts/SKILL.md", content: include_str!("builtins/grok/skills/cas-resolving-merge-conflicts/SKILL.md") },
    BuiltinFile { path: "skills/cas-to-questionnaire/SKILL.md", content: include_str!("builtins/grok/skills/cas-to-questionnaire/SKILL.md") },
];

/// OpenCode does not load a filesystem skill/agent home for its generated
/// primary agents.  Keep a process-local catalog anyway: it is the parity
/// source used by the projection and drift gate, and it makes the four
/// harnesses resolve the same required roles without pretending that an
/// OpenCode config write occurred.  Claude's canonical content is the richest
/// common source; only MCP spelling is adapted to OpenCode's `cas_<tool>`
/// sanitizer.  `OnceLock` keeps the leaked transformed strings immutable and
/// initializes them once per process.
fn project_opencode_catalog(source: &[BuiltinFile]) -> Vec<BuiltinFile> {
    source
        .iter()
        .map(|builtin| BuiltinFile {
            path: builtin.path,
            content: Box::leak(
                builtin
                    .content
                    .replace("mcp__cas__", "cas_")
                    .replace("mcp__cs__", "cas_")
                    .replace("cas__", "cas_")
                    .into_boxed_str(),
            ),
        })
        .collect()
}

static OPENCODE_BUILTIN_AGENTS: OnceLock<Vec<BuiltinFile>> = OnceLock::new();
static OPENCODE_BUILTIN_SKILLS: OnceLock<Vec<BuiltinFile>> = OnceLock::new();

fn opencode_builtin_agents() -> &'static [BuiltinFile] {
    OPENCODE_BUILTIN_AGENTS
        .get_or_init(|| project_opencode_catalog(BUILTIN_AGENTS))
        .as_slice()
}

fn opencode_builtin_skills() -> &'static [BuiltinFile] {
    OPENCODE_BUILTIN_SKILLS
        .get_or_init(|| project_opencode_catalog(BUILTIN_SKILLS))
        .as_slice()
}

/// A factory-critical capability that every harness must resolve from its own
/// catalog (cas-cc8c). The *capability* is harness-neutral; the concrete skill
/// that provides it may be a tailored twin whose directory name differs by
/// harness (e.g. Codex's `cas-codex-supervisor-checklist` vs the shared
/// `cas-supervisor-checklist`) — that intentional spelling difference is
/// encoded here, not treated as a gap.
pub struct RequiredCapability {
    /// Harness-neutral capability id (matches the AC-1 required list).
    pub id: &'static str,
    /// Skill directory (relative, e.g. `skills/cas-search`) that provides this
    /// capability for Claude / Codex / Grok respectively. `None` means the
    /// capability is intentionally not applicable to that harness — which is
    /// only legal when `note` documents why.
    pub claude: Option<&'static str>,
    pub codex: Option<&'static str>,
    pub grok: Option<&'static str>,
    /// Documented reason for any `None` above (harness-specific exemption).
    /// Must be non-empty whenever any of the three is `None`.
    pub note: &'static str,
}

/// The canonical required-capability manifest (cas-cc8c AC-1). Init/update must
/// make each harness resolve every one of these from its own catalog — never by
/// implicitly inheriting another harness's home directory. Semantic-parity tests
/// (see the test module) assert each harness catalog contains the twin's
/// `SKILL.md` for every capability, normalizing only the intentional
/// twin-spelling differences captured in the per-harness fields.
pub const REQUIRED_FACTORY_CAPABILITIES: &[RequiredCapability] = &[
    RequiredCapability {
        id: "supervisor",
        claude: Some("skills/cas-supervisor"),
        codex: Some("skills/cas-supervisor"),
        grok: Some("skills/cas-supervisor"),
        note: "",
    },
    RequiredCapability {
        id: "worker",
        claude: Some("skills/cas-worker"),
        codex: Some("skills/cas-worker"),
        grok: Some("skills/cas-worker"),
        note: "",
    },
    RequiredCapability {
        id: "task",
        claude: Some("skills/cas-task-tracking"),
        codex: Some("skills/cas-task-tracking"),
        grok: Some("skills/cas-task-tracking"),
        note: "",
    },
    RequiredCapability {
        id: "search",
        claude: Some("skills/cas-search"),
        codex: Some("skills/cas-search"),
        grok: Some("skills/cas-search"),
        note: "",
    },
    RequiredCapability {
        id: "memory",
        claude: Some("skills/cas-memory-management"),
        codex: Some("skills/cas-memory-management"),
        grok: Some("skills/cas-memory-management"),
        note: "",
    },
    RequiredCapability {
        id: "review",
        claude: Some("skills/cas-code-review"),
        codex: Some("skills/cas-code-review"),
        grok: Some("skills/cas-code-review"),
        note: "",
    },
    RequiredCapability {
        id: "verification",
        claude: Some("skills/verify-before-claim"),
        codex: Some("skills/verify-before-claim"),
        grok: Some("skills/verify-before-claim"),
        note: "",
    },
    RequiredCapability {
        id: "brainstorm",
        claude: Some("skills/cas-brainstorm"),
        codex: Some("skills/cas-brainstorm"),
        grok: Some("skills/cas-brainstorm"),
        note: "",
    },
    RequiredCapability {
        id: "ideation",
        claude: Some("skills/cas-ideate"),
        codex: Some("skills/cas-ideate"),
        grok: Some("skills/cas-ideate"),
        note: "",
    },
    RequiredCapability {
        // AC-1 "Codex supervisor-checklist ... where applicable": Claude and Grok
        // share the hooks-aware `cas-supervisor-checklist`; Codex uses its
        // tailored `cas-codex-supervisor-checklist` twin (a "no hooks"
        // compensation variant). Same capability, intentional twin spelling.
        id: "supervisor-checklist",
        claude: Some("skills/cas-supervisor-checklist"),
        codex: Some("skills/cas-codex-supervisor-checklist"),
        grok: Some("skills/cas-supervisor-checklist"),
        note: "Codex uses the cas-codex-supervisor-checklist twin (no-hooks variant); \
               Claude and Grok share the hooks-aware cas-supervisor-checklist.",
    },
];

/// General (non-factory-critical) Cassy skills that must still reach FULL parity
/// across all four harnesses (cas-20f2 — operator-requested full parity beyond
/// the minimal factory roles). Every one of these is a general-purpose skill
/// already exposed to Claude/Codex; Grok now owns a `cas__`-prefixed twin of
/// each rather than relying on `~/.claude`. Twin directory names are identical
/// across the filesystem harnesses (no tailored spelling); OpenCode projects
/// the Claude paths into its process-local catalog. The same
/// `RequiredCapability` shape applies with all three source fields set — the
/// only allowed `None` is a documented, tested runtime-prerequisite exemption.
pub const GENERAL_PARITY_CAPABILITIES: &[RequiredCapability] = &[
    RequiredCapability {
        id: "session-learn",
        claude: Some("skills/session-learn"),
        codex: Some("skills/session-learn"),
        grok: Some("skills/session-learn"),
        note: "",
    },
    RequiredCapability {
        id: "codemap",
        claude: Some("skills/codemap"),
        codex: Some("skills/codemap"),
        grok: Some("skills/codemap"),
        note: "",
    },
    RequiredCapability {
        id: "project-overview",
        claude: Some("skills/project-overview"),
        codex: Some("skills/project-overview"),
        grok: Some("skills/project-overview"),
        note: "",
    },
    RequiredCapability {
        // cas-7c93 (GH #87): sanctioned server lifecycle. Every harness needs
        // it — an agent on any CLI can leave an orphaned dev server behind.
        id: "cas-servers",
        claude: Some("skills/cas-servers"),
        codex: Some("skills/cas-servers"),
        grok: Some("skills/cas-servers"),
        note: "",
    },
    RequiredCapability {
        // GH #64: design source of truth (DESIGN.md). Same shape as
        // codemap/project-overview — every harness owns its own twin.
        id: "design-spec",
        claude: Some("skills/design-spec"),
        codex: Some("skills/design-spec"),
        grok: Some("skills/design-spec"),
        note: "",
    },
    RequiredCapability {
        // GH #65: release-notes rubric + Slack announcement workflow.
        id: "release-notes",
        claude: Some("skills/release-notes"),
        codex: Some("skills/release-notes"),
        grok: Some("skills/release-notes"),
        note: "",
    },
    RequiredCapability {
        id: "fallow",
        claude: Some("skills/fallow"),
        codex: Some("skills/fallow"),
        grok: Some("skills/fallow"),
        note: "",
    },
    RequiredCapability {
        // cas-ff2f (GH #94): the recurring GitHub Issues sweep. The hourly cron
        // prompt invokes it by name, and any harness can be the one holding the
        // pane when it fires — so all three need the twin.
        id: "cas-github-issues",
        claude: Some("skills/cas-github-issues"),
        codex: Some("skills/cas-github-issues"),
        grok: Some("skills/cas-github-issues"),
        note: "",
    },
    RequiredCapability {
        // cas-b176: report deliverables ship as self-contained HTML beside their
        // markdown source. Any harness can be the one writing the report, so all
        // three own a twin. The skill makes no CAS MCP tool calls, so the twins
        // are byte-identical to the claude source.
        id: "cas-html-reports",
        claude: Some("skills/cas-html-reports"),
        codex: Some("skills/cas-html-reports"),
        grok: Some("skills/cas-html-reports"),
        note: "",
    },
    RequiredCapability {
        // cas-1e7e: all harnesses need the same claim-first visualization guidance.
        id: "cas-dataviz",
        claude: Some("skills/cas-dataviz"),
        codex: Some("skills/cas-dataviz"),
        grok: Some("skills/cas-dataviz"),
        note: "",
    },
    RequiredCapability {
        id: "cas-nuxt-playwright",
        claude: Some("skills/cas-nuxt-playwright"),
        codex: Some("skills/cas-nuxt-playwright"),
        grok: Some("skills/cas-nuxt-playwright"),
        note: "",
    },
    RequiredCapability {
        // cas-20f2 AC-2: cas-codex-exec is a harness-neutral read-only helper
        // that shells out to the `codex exec` CLI and makes NO CAS MCP tool
        // calls. Its only prerequisite is the `codex` binary on PATH — exactly
        // the same prerequisite Claude's copy carries (the skill itself handles
        // "codex not installed" as a runtime fallback). Because the prerequisite
        // is identical across harnesses, it is INCLUDED for Grok (all three
        // Some), not exempted. The twin is byte-identical to the Claude source
        // (no `mcp__cas__` occurrences to swap).
        id: "cas-codex-exec",
        claude: Some("skills/cas-codex-exec"),
        codex: Some("skills/cas-codex-exec"),
        grok: Some("skills/cas-codex-exec"),
        note: "Runtime-bound on the `codex` CLI (same prerequisite for every \
               harness); included for Grok rather than exempted.",
    },
    RequiredCapability {
        id: "cli-routing",
        claude: Some("skills/cli-routing"),
        codex: Some("skills/cli-routing"),
        grok: Some("skills/cli-routing"),
        note: "One-shot Codex-first routing with a hard Claude account gate; \
               each harness owns the same operational guidance.",
    },
];

/// Factory-critical agent roles that every harness must define in its own agent
/// catalog (cas-cc8c AC-3). Harness-specific extras (e.g. Codex's
/// `factory-supervisor` agent, which Claude/Grok don't need because their
/// supervisor is the primary pane rather than a spawned sub-agent) are allowed
/// and are simply absent from this required set.
pub const REQUIRED_FACTORY_AGENTS: &[&str] = &[
    "agents/task-verifier.md",
    "agents/learning-reviewer.md",
    "agents/rule-reviewer.md",
    "agents/duplicate-detector.md",
    "agents/session-summarizer.md",
    "agents/git-history-analyzer.md",
    "agents/issue-intelligence-analyst.md",
];

/// The skill catalog for a harness (cas-cc8c parity helpers).
pub fn skill_catalog_for_harness(harness: SupervisorCli) -> &'static [BuiltinFile] {
    match harness {
        SupervisorCli::Claude => BUILTIN_SKILLS,
        SupervisorCli::Codex => CODEX_BUILTIN_SKILLS,
        SupervisorCli::Grok => GROK_BUILTIN_SKILLS,
        SupervisorCli::OpenCode => opencode_builtin_skills(),
    }
}

/// The agent catalog for a harness (cas-cc8c parity helpers).
pub fn agent_catalog_for_harness(harness: SupervisorCli) -> &'static [BuiltinFile] {
    match harness {
        SupervisorCli::Claude => BUILTIN_AGENTS,
        SupervisorCli::Codex => CODEX_BUILTIN_AGENTS,
        SupervisorCli::Grok => GROK_BUILTIN_AGENTS,
        SupervisorCli::OpenCode => opencode_builtin_agents(),
    }
}

/// The skill directory a capability requires for `harness`, or `None` when it is
/// intentionally not applicable to that harness.
pub fn required_dir_for(cap: &RequiredCapability, harness: SupervisorCli) -> Option<&'static str> {
    match harness {
        SupervisorCli::Claude => cap.claude,
        SupervisorCli::Codex => cap.codex,
        SupervisorCli::Grok => cap.grok,
        // OpenCode projects the Claude catalog in-process and normalizes its
        // tool names; no user-level skill directory is written.
        SupervisorCli::OpenCode => cap.claude,
    }
}

/// Check if a file is managed by Cassy (has `managed_by: cas` in frontmatter)
pub fn is_managed_by_cas(content: &str) -> bool {
    // Check frontmatter for managed_by: cas
    if let Some(stripped) = content.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            let frontmatter = &content[3..3 + end];
            return frontmatter.contains("managed_by: cas")
                || frontmatter.contains("managed_by: \"cas\"");
        }
    }
    false
}

/// Preview what would change for a built-in file (dry-run)
/// Returns Some((old_content, new_content)) if file would be updated
pub fn preview_builtin(
    builtin: &BuiltinFile,
    target_dir: &Path,
) -> std::io::Result<Option<(String, String)>> {
    let target = target_dir.join(builtin.path);
    let content = builtin.content;

    if target.exists() {
        let existing = std::fs::read_to_string(&target)?;

        // Only update if managed by Cassy
        if !is_managed_by_cas(&existing) && !is_managed_by_cas(content) {
            return Ok(None);
        }

        // Check if content is the same
        if existing == content {
            return Ok(None);
        }

        Ok(Some((existing, content.to_string())))
    } else {
        // New file
        Ok(Some((String::new(), content.to_string())))
    }
}

/// Outcome of a single `sync_builtin_detailed` call. The interesting
/// variant is `SkippedNotManaged` — that is the cas-4900 silent-skip
/// case (target exists, content differs from source, but the
/// managed-by-cas gate refused to write because neither side carries the
/// frontmatter marker). Callers that summarize a sync report should
/// surface these so the staleness becomes observable instead of silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Wrote a new file (target did not exist on disk).
    Created,
    /// Overwrote an existing file (content differed and the managed-by
    /// gate let us through).
    Updated,
    /// Target existed and content already matched source byte-for-byte.
    /// Happy-path no-op.
    Unchanged,
    /// Target exists, content differs from source, but neither version
    /// carries `managed_by: cas` in its frontmatter — the gate kept us
    /// from clobbering. **This is the visible-staleness signal**
    /// (cas-4900): the file at the destination is provably stale and
    /// the caller should surface it in CLI output.
    SkippedNotManaged,
    /// The file is a reference owned by a managed builtin skill, but its
    /// destination does not match the last content Cassy synced. Preserve the
    /// local content and surface the conflict instead of silently clobbering
    /// an intentional customization.
    SkippedModifiedReference,
}

impl SyncOutcome {
    /// True for the two write-bearing outcomes (`Created` / `Updated`).
    /// Preserves the back-compat surface for callers that previously
    /// read `sync_builtin` as a plain `bool`.
    pub fn wrote(self) -> bool {
        matches!(self, SyncOutcome::Created | SyncOutcome::Updated)
    }
}

/// Rich variant of [`sync_builtin`]: returns a [`SyncOutcome`] so the
/// caller can distinguish silent-skip (stale-source-not-managed) from
/// happy-path no-op, which the legacy `bool` return value collapsed
/// into the same value and produced the cas-4900 silent-staleness
/// regression.
pub fn sync_builtin_detailed(
    builtin: &BuiltinFile,
    target_dir: &Path,
) -> std::io::Result<SyncOutcome> {
    let target = target_dir.join(builtin.path);
    let content = builtin.content;

    // Create parent directories
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Check if file exists and whether we should overwrite
    if target.exists() {
        let existing = std::fs::read_to_string(&target)?;

        // Only overwrite if it's managed by Cassy
        if !is_managed_by_cas(&existing) && !is_managed_by_cas(content) {
            // Neither version is managed — don't overwrite user content.
            // Distinguish "content actually differs" (the silent-staleness
            // case worth warning about) from "content matches anyway"
            // (genuine no-op): emit `SkippedNotManaged` only on the
            // former so callers can warn-and-link the user to the
            // managed-by-cas marker fix.
            if existing == content {
                return Ok(SyncOutcome::Unchanged);
            }
            tracing::warn!(
                path = %builtin.path,
                "sync_builtin: silent skip — destination differs from source but \
                 neither side carries `managed_by: cas` frontmatter; file is stale. \
                 Add `managed_by: cas` to the source frontmatter to enable updates \
                 (cas-4900)."
            );
            return Ok(SyncOutcome::SkippedNotManaged);
        }

        // Check if content is the same
        if existing == content {
            return Ok(SyncOutcome::Unchanged);
        }

        std::fs::write(&target, content)?;
        Ok(SyncOutcome::Updated)
    } else {
        std::fs::write(&target, content)?;
        Ok(SyncOutcome::Created)
    }
}

/// Sync a built-in file to the target directory.
/// Returns true if file was written/updated.
///
/// Back-compat wrapper over [`sync_builtin_detailed`]; new call sites
/// should prefer the detailed variant so they can surface the
/// `SkippedNotManaged` case (cas-4900). Internal callers like
/// [`sync_all_builtins_inner`] already migrated.
pub fn sync_builtin(builtin: &BuiltinFile, target_dir: &Path) -> std::io::Result<bool> {
    Ok(sync_builtin_detailed(builtin, target_dir)?.wrote())
}

const BUILTIN_REFERENCE_STATE_FILE: &str = ".cas-builtin-reference-state.json";

fn builtin_reference_state_path(target_dir: &Path) -> std::path::PathBuf {
    let project_cache = target_dir
        .parent()
        .map(|parent| parent.join(".cas").join("cache"))
        .filter(|cache| cache.parent().is_some_and(Path::is_dir));
    if let Some(cache) = project_cache {
        let harness = target_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("harness")
            .trim_start_matches('.');
        return cache
            .join("builtin-reference-state")
            .join(format!("{harness}.json"));
    }
    target_dir.join(BUILTIN_REFERENCE_STATE_FILE)
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BuiltinReferenceState {
    /// Schema version for forward-compatible state migrations.
    version: u8,
    /// SHA-256 of the source content Cassy last installed at each reference path.
    files: BTreeMap<String, String>,
    /// References the user deleted before project sync. Database skill sync
    /// may rehydrate an old copy before builtin sync runs, so retain deletion
    /// as explicit consent to replace that copy once.
    #[serde(default)]
    replace_on_next_sync: BTreeSet<String>,
    /// References the most recent sync refused to update because their content
    /// matches neither the recorded baseline nor any version Cassy has shipped
    /// (cas-0c0a). Persisted so SessionStart can surface the skip where
    /// operators actually look — a `tracing::warn` inside an unattended
    /// `cas update --sync` is seen by nobody.
    #[serde(default)]
    skipped_references: BTreeMap<String, String>,
}

impl BuiltinReferenceState {
    fn load(target_dir: &Path) -> std::io::Result<Self> {
        let path = builtin_reference_state_path(target_dir);
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(state) => Ok(state),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "builtin reference sync state is unreadable; treating all divergent \
                         references as local modifications"
                    );
                    Ok(Self {
                        version: 1,
                        ..Self::default()
                    })
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                version: 1,
                ..Self::default()
            }),
            Err(error) => Err(error),
        }
    }

    fn save(&self, target_dir: &Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let path = builtin_reference_state_path(target_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{content}\n"))
    }
}

/// Remember body-owned references that are absent before database skill sync.
/// The database layer can rehydrate an older copy before builtin sync runs;
/// this one-shot marker preserves deletion as explicit acceptance of the
/// current embedded Cassy version.
pub fn mark_missing_owned_references_for_replacement(
    harness: SupervisorCli,
    target_dir: &Path,
) -> std::io::Result<usize> {
    let skills = match harness {
        SupervisorCli::Claude => BUILTIN_SKILLS,
        SupervisorCli::Codex => CODEX_BUILTIN_SKILLS,
        SupervisorCli::Grok => GROK_BUILTIN_SKILLS,
        // OpenCode keeps builtin references in the generated primary-agent
        // projection; its user-level config has no reference ledger.
        SupervisorCli::OpenCode => opencode_builtin_skills(),
    };
    let mut state = BuiltinReferenceState::load(target_dir)?;
    let mut marked = 0;
    for builtin in skills {
        if is_reference_owned_by_managed_skill(builtin, skills)
            && !target_dir.join(builtin.path).exists()
            && state
                .replace_on_next_sync
                .insert(builtin.path.to_string())
        {
            marked += 1;
        }
    }
    if marked > 0 {
        state.save(target_dir)?;
    }
    Ok(marked)
}

fn builtin_content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

/// SHA-256 of every previously shipped version of each builtin reference file
/// (cas-0c0a). Regenerated by `scripts/gen-builtin-reference-history.sh`.
///
/// Without this, a destination installed before the baseline ledger existed
/// (Jul 2026) had no recorded baseline, so pristine-but-old Cassy content was
/// indistinguishable from a local customization and was preserved — silently,
/// forever. Matching the destination against known shipped hashes recovers the
/// distinction: a known hash is an old Cassy version (safe to replace), an
/// unknown one is a real local edit (must be preserved).
const BUILTIN_REFERENCE_HISTORY_JSON: &str = include_str!("builtins/reference-history.json");

#[derive(Debug, Deserialize)]
struct BuiltinReferenceHistory {
    #[serde(default)]
    files: BTreeMap<String, BTreeSet<String>>,
}

fn builtin_reference_history() -> &'static BuiltinReferenceHistory {
    static HISTORY: std::sync::OnceLock<BuiltinReferenceHistory> = std::sync::OnceLock::new();
    HISTORY.get_or_init(|| {
        serde_json::from_str(BUILTIN_REFERENCE_HISTORY_JSON).unwrap_or_else(|error| {
            tracing::warn!(
                %error,
                "embedded builtin reference history is unparseable; pre-ledger references \
                 will be preserved as possible local customizations"
            );
            BuiltinReferenceHistory {
                files: BTreeMap::new(),
            }
        })
    })
}

/// True when `content_hash` matches a version of `path` that Cassy itself
/// shipped at some point — i.e. the destination is provably stale Cassy content
/// rather than a local edit.
fn is_shipped_builtin_reference_version(path: &str, content_hash: &str) -> bool {
    #[cfg(test)]
    {
        if let Some(found) = tests::history_override_lookup(path, content_hash) {
            return found;
        }
    }
    builtin_reference_history()
        .files
        .get(path)
        .is_some_and(|hashes| hashes.contains(content_hash))
}

/// A reference is owned by its skill directory when that directory has a
/// cataloged, managed `SKILL.md`. This makes ownership automatic for newly
/// added reference files instead of relying on an easy-to-forget per-file
/// frontmatter marker.
fn is_reference_owned_by_managed_skill(builtin: &BuiltinFile, skills: &[BuiltinFile]) -> bool {
    let Some(relative) = builtin.path.strip_prefix("skills/") else {
        return false;
    };
    let Some((skill_dir, child_path)) = relative.split_once('/') else {
        return false;
    };
    if !child_path.starts_with("references/") {
        return false;
    }

    let body_path = format!("skills/{skill_dir}/SKILL.md");
    skills
        .iter()
        .find(|candidate| candidate.path == body_path)
        .is_some_and(|body| is_managed_by_cas(body.content))
}

fn sync_owned_reference(
    builtin: &BuiltinFile,
    target_dir: &Path,
    state: &mut BuiltinReferenceState,
) -> std::io::Result<SyncOutcome> {
    let target = target_dir.join(builtin.path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let source_hash = builtin_content_hash(builtin.content);
    let replacement_requested = state.replace_on_next_sync.remove(builtin.path);
    match std::fs::read_to_string(&target) {
        Ok(existing) if existing == builtin.content => {
            state.files.insert(builtin.path.to_string(), source_hash);
            state.skipped_references.remove(builtin.path);
            Ok(SyncOutcome::Unchanged)
        }
        Ok(existing) => {
            let destination_hash = builtin_content_hash(&existing);
            let matches_baseline = replacement_requested
                || state
                    .files
                    .get(builtin.path)
                    .is_some_and(|baseline| baseline == &destination_hash);
            // cas-0c0a: no baseline is not evidence of a local edit. If the
            // destination content is a version Cassy previously shipped, it is
            // stale Cassy content and must be upgraded — otherwise every
            // pre-ledger install keeps its Jun-2026 copy forever.
            let is_stale_shipped_version = !matches_baseline
                && is_shipped_builtin_reference_version(builtin.path, &destination_hash);
            if !matches_baseline && !is_stale_shipped_version {
                tracing::warn!(
                    path = %builtin.path,
                    "builtin skill reference differs from its last Cassy-synced content and from \
                     every version Cassy has shipped; preserving the destination as a local \
                     customization. Review it, then delete the destination and rerun \
                     `cas update --sync` to accept the Cassy version."
                );
                state
                    .skipped_references
                    .insert(builtin.path.to_string(), destination_hash);
                return Ok(SyncOutcome::SkippedModifiedReference);
            }
            if is_stale_shipped_version {
                tracing::info!(
                    path = %builtin.path,
                    "builtin skill reference had no recorded baseline but matches a previously \
                     shipped Cassy version; upgrading and baselining it (cas-0c0a)."
                );
            }

            std::fs::write(&target, builtin.content)?;
            state.files.insert(builtin.path.to_string(), source_hash);
            state.skipped_references.remove(builtin.path);
            Ok(SyncOutcome::Updated)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&target, builtin.content)?;
            state.files.insert(builtin.path.to_string(), source_hash);
            state.skipped_references.remove(builtin.path);
            Ok(SyncOutcome::Created)
        }
        Err(error) => Err(error),
    }
}

/// References the last sync preserved instead of updating, grouped by harness
/// (`claude` / `codex` / `grok`), read from the per-harness baseline ledgers
/// under `<cas_root>/cache/builtin-reference-state/` (cas-0c0a).
///
/// This is the data behind the SessionStart staleness banner: it is the only
/// place a skip is durable, because `cas update --sync` writes its warning to
/// stdout/tracing that unattended runs discard.
pub fn skipped_owned_references(cas_root: &Path) -> BTreeMap<String, Vec<String>> {
    let dir = cas_root.join("cache").join("builtin-reference-state");
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(harness) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<BuiltinReferenceState>(&content) else {
            continue;
        };
        if state.skipped_references.is_empty() {
            continue;
        }
        out.insert(
            harness.to_string(),
            state.skipped_references.keys().cloned().collect(),
        );
    }
    out
}

/// Sync all built-in files to the target directory
fn sync_all_builtins_inner(
    target_dir: &Path,
    agents: &[BuiltinFile],
    skills: &[BuiltinFile],
) -> std::io::Result<SyncResult> {
    let mut result = SyncResult::default();
    let mut reference_state = BuiltinReferenceState::load(target_dir)?;

    // Sync agents
    for builtin in agents {
        match sync_builtin_detailed(builtin, target_dir)? {
            SyncOutcome::Created | SyncOutcome::Updated => {
                result.agents_updated += 1;
                result.updated_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedNotManaged => {
                result.skipped_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedModifiedReference => {
                result
                    .modified_reference_files
                    .push(builtin.path.to_string());
            }
            SyncOutcome::Unchanged => {}
        }
    }

    // Sync skills
    for builtin in skills {
        let outcome = if is_reference_owned_by_managed_skill(builtin, skills) {
            sync_owned_reference(builtin, target_dir, &mut reference_state)?
        } else {
            sync_builtin_detailed(builtin, target_dir)?
        };
        match outcome {
            SyncOutcome::Created | SyncOutcome::Updated => {
                result.skills_updated += 1;
                result.updated_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedNotManaged => {
                result.skipped_files.push(builtin.path.to_string());
            }
            SyncOutcome::SkippedModifiedReference => {
                result
                    .modified_reference_files
                    .push(builtin.path.to_string());
            }
            SyncOutcome::Unchanged => {}
        }
    }

    reference_state.save(target_dir)?;
    Ok(result)
}

/// Sync built-in Workflow scripts to the target directory.
///
/// Unlike skills and agents (which use the `managed_by: cas` gate), workflow
/// scripts are always force-written — they are machine-generated JS files that
/// users should not hand-edit. A workflow that diverges from the builtin is
/// always replaced on sync.
///
/// Counts are returned on `result.skills_updated` (workflow scripts don't have
/// their own counter; they are a minor surface relative to skills).
fn sync_workflows(
    target_dir: &Path,
    workflows: &[BuiltinFile],
    result: &mut SyncResult,
) -> std::io::Result<()> {
    for wf in workflows {
        let target = target_dir.join(wf.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let needs_write = match std::fs::read_to_string(&target) {
            Ok(existing) => existing != wf.content,
            Err(_) => true, // file absent or unreadable → create
        };
        if needs_write {
            std::fs::write(&target, wf.content)?;
            result.skills_updated += 1;
            result.updated_files.push(wf.path.to_string());
        }
    }
    Ok(())
}

/// Sync all built-in files to .claude/ directory
pub fn sync_all_builtins(claude_dir: &Path) -> std::io::Result<SyncResult> {
    let mut result = sync_all_builtins_inner(claude_dir, BUILTIN_AGENTS, BUILTIN_SKILLS)?;
    sync_workflows(claude_dir, BUILTIN_WORKFLOWS, &mut result)?;
    Ok(result)
}

/// Sync all built-in files to .codex/ directory
pub fn sync_all_codex_builtins(codex_dir: &Path) -> std::io::Result<SyncResult> {
    sync_all_builtins_inner(codex_dir, CODEX_BUILTIN_AGENTS, CODEX_BUILTIN_SKILLS)
}

/// Sync all built-in files to .grok/ directory (EPIC cas-8888, Phase 5).
pub fn sync_all_grok_builtins(grok_dir: &Path) -> std::io::Result<SyncResult> {
    sync_all_builtins_inner(grok_dir, GROK_BUILTIN_AGENTS, GROK_BUILTIN_SKILLS)
}

/// Sync all built-ins for a specific harness.
pub fn sync_all_builtins_for_harness(
    harness: SupervisorCli,
    target_dir: &Path,
) -> std::io::Result<SyncResult> {
    match harness {
        SupervisorCli::Claude => sync_all_builtins(target_dir),
        SupervisorCli::Codex => sync_all_codex_builtins(target_dir),
        // EPIC cas-8888 (cas-6f46, Phase 5): dedicated GROK_BUILTIN_AGENTS/
        // GROK_BUILTIN_SKILLS set, cas__-prefixed (no mcp__ wrapper).
        SupervisorCli::Grok => sync_all_grok_builtins(target_dir),
        // OpenCode receives this catalog through OPENCODE_CONFIG_CONTENT; no
        // user-level skill tree is written by the sync command.
        SupervisorCli::OpenCode => Ok(SyncResult::default()),
    }
}

/// Collect the set of skill directory names (`cas-foo`) owned by a builtins
/// slice. Builtin skill paths look like `skills/<dir>/SKILL.md` (or a nested
/// `references/...`); we extract `<dir>` so the prune below can recognize the
/// dirs Cassy just wrote and never remove them.
fn builtin_skill_dir_names(skills: &[BuiltinFile]) -> HashSet<String> {
    skills
        .iter()
        .filter_map(|b| b.path.strip_prefix("skills/"))
        .filter_map(|rest| rest.split('/').next())
        .map(|s| s.to_string())
        .collect()
}

/// Prune stale, non-managed `cas-*` skill directories from a `skills/` dir.
///
/// This mirrors the project-level prune in `SkillSyncer::sync_all`
/// (`cas-cli/src/sync/skills.rs`): a directory is removed only when ALL of
/// these hold:
///   1. its name is `cas-*` prefixed (we never touch user-authored skills),
///   2. it is not one of the builtin skill dirs we just wrote (`keep`), and
///   3. its `SKILL.md` is genuinely absent OR present-and-unmanaged (no
///      `managed_by: cas` marker). Any other read error (permission denied,
///      I/O) preserves the directory — we only delete when we can positively
///      confirm it is not a managed builtin.
///
/// The managed-by check is the critical safety net: a freshly-synced builtin
/// always carries the marker, so even if `keep` is somehow incomplete the
/// builtin survives. Non-`cas-` dirs are left untouched. Used by
/// `cas update --user` (`sync_user_builtins`) so that legacy
/// orphans like `cas-playwright-debug` — which the project-level sync already
/// prunes but the user-level path historically never did — are removed from
/// `~/.claude/skills` and `~/.codex/skills` on every downstream host.
///
/// Returns the names of the directories that were removed.
pub fn prune_stale_cas_skill_dirs(
    skills_dir: &Path,
    keep: &HashSet<String>,
) -> std::io::Result<Vec<String>> {
    let mut removed = Vec::new();
    if !skills_dir.exists() {
        return Ok(removed);
    }

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Only ever touch cas-* dirs we are not currently writing.
        if !name.starts_with("cas-") || keep.contains(&name) {
            continue;
        }

        // Only delete when we can positively confirm this is not a managed
        // builtin: SKILL.md is either genuinely absent, or present without the
        // managed_by: cas marker. A permission/I/O read error (anything other
        // than NotFound) preserves the dir — never destroy on uncertainty.
        let skill_file = path.join("SKILL.md");
        let safe_to_remove = match std::fs::read_to_string(&skill_file) {
            Ok(content) => !is_managed_by_cas(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        if !safe_to_remove {
            continue;
        }

        std::fs::remove_dir_all(&path)?;
        removed.push(name);
    }

    Ok(removed)
}

/// Prune stale non-managed `cas-*` skill dirs from a harness's user-level
/// `skills/` directory, keeping the builtins that harness owns. Thin wrapper
/// over [`prune_stale_cas_skill_dirs`] that selects the right builtin set.
pub fn prune_stale_user_skills_for_harness(
    harness: SupervisorCli,
    harness_dir: &Path,
) -> std::io::Result<Vec<String>> {
    let builtins = match harness {
        SupervisorCli::Claude => BUILTIN_SKILLS,
        SupervisorCli::Codex => CODEX_BUILTIN_SKILLS,
        SupervisorCli::Grok => GROK_BUILTIN_SKILLS,
        // OpenCode has no user-level skill tree; the process-local projection
        // is installed by its generated primary-agent config.
        SupervisorCli::OpenCode => &[],
    };
    let keep = builtin_skill_dir_names(builtins);
    prune_stale_cas_skill_dirs(&harness_dir.join("skills"), &keep)
}

#[derive(Default, Debug)]
pub struct SyncResult {
    pub agents_updated: usize,
    pub skills_updated: usize,
    pub updated_files: Vec<String>,
    /// Paths (relative to `target_dir`) whose source content differs from
    /// the on-disk destination, but the managed-by gate refused to
    /// overwrite because neither version carries `managed_by: cas`. This
    /// is the cas-4900 silent-staleness signal — callers like
    /// `cas update --sync` should surface these as warnings so the user
    /// can either add the marker to the source or accept the staleness
    /// knowingly. Distinct from "no-op" (`Unchanged`) where source and
    /// destination already match.
    pub skipped_files: Vec<String>,
    /// Body-owned builtin references whose destination differs from the
    /// last Cassy-synced baseline. These are preserved as possible intentional
    /// local customizations and must be surfaced to the user.
    pub modified_reference_files: Vec<String>,
}

impl SyncResult {
    pub fn total_updated(&self) -> usize {
        self.agents_updated + self.skills_updated
    }

    /// True when the sync left at least one file behind because the
    /// managed-by gate would not let us overwrite. cas-4900.
    pub fn has_silent_skips(&self) -> bool {
        !self.skipped_files.is_empty()
    }

    pub fn has_modified_references(&self) -> bool {
        !self.modified_reference_files.is_empty()
    }
}

/// A pending builtin change for dry-run preview
#[derive(Debug)]
pub struct BuiltinChange {
    pub path: String,
    pub old_content: String,
    pub new_content: String,
    pub is_new: bool,
}

/// Preview all built-in file changes (dry-run mode)
pub fn preview_all_builtins(claude_dir: &Path) -> std::io::Result<Vec<BuiltinChange>> {
    let mut changes = Vec::new();

    let all_builtins = BUILTIN_AGENTS.iter().chain(BUILTIN_SKILLS.iter());

    for builtin in all_builtins {
        if let Some((old, new)) = preview_builtin(builtin, claude_dir)? {
            changes.push(BuiltinChange {
                path: builtin.path.to_string(),
                old_content: old.clone(),
                new_content: new,
                is_new: old.is_empty(),
            });
        }
    }

    Ok(changes)
}

/// Preview all Codex built-in file changes (dry-run mode)
pub fn preview_all_codex_builtins(codex_dir: &Path) -> std::io::Result<Vec<BuiltinChange>> {
    let mut changes = Vec::new();

    let all_builtins = CODEX_BUILTIN_AGENTS
        .iter()
        .chain(CODEX_BUILTIN_SKILLS.iter());

    for builtin in all_builtins {
        if let Some((old, new)) = preview_builtin(builtin, codex_dir)? {
            changes.push(BuiltinChange {
                path: builtin.path.to_string(),
                old_content: old.clone(),
                new_content: new,
                is_new: old.is_empty(),
            });
        }
    }

    Ok(changes)
}

/// Preview all Grok built-in file changes (dry-run mode) (EPIC cas-8888,
/// Phase 5).
pub fn preview_all_grok_builtins(grok_dir: &Path) -> std::io::Result<Vec<BuiltinChange>> {
    let mut changes = Vec::new();

    let all_builtins = GROK_BUILTIN_AGENTS.iter().chain(GROK_BUILTIN_SKILLS.iter());

    for builtin in all_builtins {
        if let Some((old, new)) = preview_builtin(builtin, grok_dir)? {
            changes.push(BuiltinChange {
                path: builtin.path.to_string(),
                old_content: old.clone(),
                new_content: new,
                is_new: old.is_empty(),
            });
        }
    }

    Ok(changes)
}

/// Preview all built-ins for a specific harness.
pub fn preview_all_builtins_for_harness(
    harness: SupervisorCli,
    target_dir: &Path,
) -> std::io::Result<Vec<BuiltinChange>> {
    match harness {
        SupervisorCli::Claude => preview_all_builtins(target_dir),
        SupervisorCli::Codex => preview_all_codex_builtins(target_dir),
        SupervisorCli::Grok => preview_all_grok_builtins(target_dir),
        // OpenCode has no user-level skill tree to preview; its process-local
        // projection is rendered at launch.
        SupervisorCli::OpenCode => Ok(Vec::new()),
    }
}

// =============================================================================
// Factory Guidance Functions (for HooksConfig)
// =============================================================================

/// Extract the body content from a skill markdown file, stripping YAML frontmatter
///
/// Skill files have the format:
/// ```markdown
/// ---
/// name: skill-name
/// description: ...
/// ---
///
/// # Title
/// Content...
/// ```
///
/// This function returns everything after the closing `---` of the frontmatter.
pub fn extract_body(content: &str) -> &str {
    // Find the opening ---
    let Some(start) = content.find("---") else {
        return content;
    };

    // Find the closing --- (after the opening one)
    let after_first = &content[start + 3..];
    let Some(end_offset) = after_first.find("---") else {
        return content;
    };

    // Return everything after the closing ---
    let body_start = start + 3 + end_offset + 3;
    content[body_start..].trim_start()
}

/// Claude Code harness truncation point for inline supervisor guidance.
pub(crate) const SUPERVISOR_GUIDANCE_HARD_CEILING_BYTES: usize = 8_192;

/// Early-warning margin that preserves room for launch wrappers and banners.
pub(crate) const SUPERVISOR_GUIDANCE_SOFT_CAP_BYTES: usize = 8_000;

/// Get the supervisor guidance injected at factory SessionStart.
///
/// Returns only the trimmed supervisor SKILL.md body. The checklist
/// (`cas-supervisor-checklist`) is a separate skill invocable via
/// `/cas-supervisor-checklist` — bundling it pushed the SessionStart payload
/// over the ~10KB Claude Code harness cap (measured by cas-ecd5, 2026-06-01),
/// causing the full briefing to be silently replaced with a 2KB preview.
/// task-tracking, memory, and search are autonomous skills the agent invokes
/// on demand via the Skill tool — same rationale.
pub fn supervisor_guidance() -> String {
    extract_body(SUPERVISOR_GUIDE).to_string()
}

/// Get the worker guidance injected at factory SessionStart.
///
/// Returns only the worker SKILL.md. task-tracking/memory/search load on
/// demand — same rationale as `supervisor_guidance`.
pub fn worker_guidance() -> String {
    extract_body(WORKER_GUIDE).to_string()
}

#[cfg(test)]
mod tests {
    use crate::builtins::*;

    thread_local! {
        /// Test-only stand-in for the embedded shipped-version history
        /// (cas-0c0a), so grandfathering can be exercised with synthetic
        /// builtin paths instead of real historical file contents.
        static HISTORY_OVERRIDE: std::cell::RefCell<Option<BTreeMap<String, BTreeSet<String>>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// `Some(hit)` when a test has installed an override; `None` means "fall
    /// through to the embedded history".
    pub(super) fn history_override_lookup(path: &str, content_hash: &str) -> Option<bool> {
        HISTORY_OVERRIDE.with(|cell| {
            cell.borrow().as_ref().map(|files| {
                files
                    .get(path)
                    .is_some_and(|hashes| hashes.contains(content_hash))
            })
        })
    }

    /// Declare `contents` as previously shipped versions of `path` for the rest
    /// of this test thread.
    fn set_history_override(path: &str, contents: &[&str]) {
        let hashes: BTreeSet<String> = contents.iter().map(|c| builtin_content_hash(c)).collect();
        HISTORY_OVERRIDE.with(|cell| {
            *cell.borrow_mut() = Some(BTreeMap::from([(path.to_string(), hashes)]));
        });
    }

    fn clear_history_override() {
        HISTORY_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
    }

    fn extract_js_function(source: &str, name: &str) -> String {
        let needle = format!("function {name}(");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("missing JS function {name}"));
        let after_name = &source[start..];
        let open_rel = after_name
            .find('{')
            .unwrap_or_else(|| panic!("missing opening brace for JS function {name}"));
        let open = start + open_rel;
        let mut depth = 0usize;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return source[start..=open + offset].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("missing closing brace for JS function {name}");
    }

    #[test]
    fn test_extract_body_with_frontmatter() {
        let content = r#"---
name: test
description: A test skill
---

# Test Skill

This is the body content."#;

        let body = extract_body(content);
        assert!(body.starts_with("# Test Skill"));
        assert!(body.contains("This is the body content."));
        assert!(!body.contains("name: test"));
    }

    #[test]
    fn test_extract_body_no_frontmatter() {
        let content = "# Just Content\n\nNo frontmatter here.";
        let body = extract_body(content);
        assert_eq!(body, content);
    }

    #[test]
    fn test_supervisor_guidance_loads() {
        let guide = supervisor_guidance();
        assert!(guide.contains("Factory Supervisor"));
        assert!(!guide.contains("managed_by:"));
        // Checklist must NOT be bundled — it loads separately via /cas-supervisor-checklist.
        assert!(
            !guide.contains("Supervisor Checklist"),
            "should NOT bundle checklist — invocable separately via /cas-supervisor-checklist"
        );
        // task-tracking/memory/search are autonomous skills, not bundled.
        assert!(
            !guide.contains("Cassy Task Tracking"),
            "should NOT bundle task-tracking — loads on demand"
        );
        assert!(
            !guide.contains("Cassy Memory Management"),
            "should NOT bundle memory — loads on demand"
        );
    }

    /// All 6 Hard Rules must appear verbatim in the supervisor briefing.
    /// These keywords are the ones confirmed present in the model-visible
    /// hook_additional_context bytes after the harness cap trim (cas-5e4b).
    #[test]
    fn test_supervisor_guidance_hard_rules() {
        let guide = supervisor_guidance();
        for keyword in [
            "AskUserQuestion",
            "SendMessage",
            "coordination",
            "Never close",
            "Never implement",
            "Never monitor",
            "End your turn",
        ] {
            assert!(
                guide.contains(keyword),
                "supervisor_guidance() missing Hard Rule keyword: {keyword:?}"
            );
        }
    }

    /// cas-edf4: the codex-flavored supervisor guide carries the same
    /// deliberate-tiering hard rule as the Claude copy (cas-c093) — the
    /// codex copy has no byte-cap test gating it, so this is the guard
    /// against the two surfaces silently drifting back apart.
    #[test]
    fn test_codex_supervisor_guidance_mirrors_tiering_rule() {
        let codex_guide = include_str!("builtins/codex/skills/cas-supervisor.md");
        for keyword in [
            "Tier every spawn",
            "never fleet-default",
            "light",
            "standard",
            "heavy",
            "frontier",
            "model-selection.md",
            // cas-b342/cas-96ea: Codex-first tier table + exception/capacity lanes in the body.
            "Codex-first tiers",
            "grok/grok-composer-2.5-fast/low",
            "codex/gpt-5.6-sol/high",
            "codex/gpt-5.6-luna/xhigh",
            "Opus",
            "capacity route",
        ] {
            assert!(
                codex_guide.contains(keyword),
                "codex cas-supervisor.md missing tiering-rule keyword: {keyword:?}"
            );
        }
        // Quick Start step 6 must show a tiered mix, not a single default line.
        assert!(
            codex_guide.contains("tiered mix"),
            "codex cas-supervisor.md Quick Start must not read as a single default spawn recipe"
        );
    }

    /// cas-b342: the three supervisor bodies must be semantically identical
    /// apart from the intentional per-harness tool prefix (mcp__cas__ /
    /// mcp__cs__ / cas__) and the Grok Heterogeneous-Teams section title. This
    /// pins routing examples (tier table, Quick Start spawn recipes, the
    /// heterogeneous complete-call) to full explicit controls across all three
    /// harnesses — a condensed or drifted example on one twin now fails CI.
    #[test]
    fn test_supervisor_bodies_normalized_consistent_across_harnesses() {
        let claude = SUPERVISOR_GUIDE;
        let codex = include_str!("builtins/codex/skills/cas-supervisor.md");
        let grok = include_str!("builtins/grok/skills/cas-supervisor.md");

        // Claude -> Codex is a pure tool-prefix mirror.
        assert_eq!(
            claude.replace("mcp__cas__", "mcp__cs__"),
            codex,
            "codex cas-supervisor.md must equal the Claude body apart from the \
             mcp__cas__/mcp__cs__ tool prefix"
        );

        // Claude -> Grok differs only by the cas__ prefix and the intentional
        // Heterogeneous-Teams section title (Grok supervisors lead a different
        // fleet). Normalize both, then require exact equality.
        let claude_as_grok = claude
            .replace("mcp__cas__", "cas__")
            .replace(
                "## Heterogeneous Teams (Claude supervisor + Codex workers)",
                "## Heterogeneous Teams (Grok supervisor + Claude/Codex workers)",
            );
        assert_eq!(
            claude_as_grok, grok,
            "grok cas-supervisor.md must equal the Claude body apart from the cas__ \
             tool prefix and the intentional Heterogeneous-Teams section title"
        );

        // The shared body must carry the full explicit spawn recipes (not a
        // condensed `effort=high`-only heavy example) on every twin.
        for (label, body) in [("claude", claude), ("codex", codex), ("grok", grok)] {
            assert!(
                body.contains("model=gpt-5.6-sol effort=high` for a heavy one"),
                "{label} cas-supervisor.md must give the heavy spawn as a complete recipe"
            );
            assert!(
                body.contains("pass complete `cli=`, `model=`, and `effort=` controls"),
                "{label} cas-supervisor.md heterogeneous example must be a complete call"
            );
        }
    }

    /// The checklist is a separate skill invocable via /cas-supervisor-checklist.
    /// Bundling it into supervisor_guidance() would push the SessionStart
    /// payload over the ~10KB harness cap (cas-ecd5, 2026-06-01).
    #[test]
    fn test_supervisor_guidance_no_checklist() {
        let guide = supervisor_guidance();
        assert!(
            !guide.contains("# Supervisor Checklist"),
            "supervisor_guidance() must not inline the checklist — \
             it is invocable separately via /cas-supervisor-checklist"
        );
        // Cross-check: the checklist skill itself must still exist.
        let checklist = extract_body(CHECKLIST_GUIDE);
        assert!(
            checklist.contains("# Supervisor Checklist"),
            "CHECKLIST_GUIDE must still contain its content (invocable on demand)"
        );
    }

    /// SessionStart additionalContext gets truncated by the Claude Code harness
    /// once the payload exceeds its ~10KB threshold (measured empirically by
    /// cas-ecd5, 2026-06-01). The hard ceiling is 8192 bytes (~2KB headroom for
    /// SessionStart banners: codemap freshness, agent identity, WIP banner). We
    /// assert a slightly tighter 8000-byte soft cap (cas-b342) so a routine
    /// punctuation/wording edit can't silently detonate the 8192 ceiling in CI —
    /// ~192 bytes of guaranteed slack. Over the soft cap: move content into
    /// cas-supervisor/references/ rather than inlining it in cas-supervisor.md.
    /// See memory `project_session_start_truncation.md`.
    #[test]
    fn test_supervisor_guidance_under_8kb() {
        let guide = supervisor_guidance();
        assert!(
            guide.len() < SUPERVISOR_GUIDANCE_HARD_CEILING_BYTES,
            "supervisor_guidance is {} bytes — over the {SUPERVISOR_GUIDANCE_HARD_CEILING_BYTES}B SessionStart ceiling. \
             Move content into cas-supervisor/references/ instead of \
             inlining it in cas-supervisor.md.",
            guide.len()
        );
        assert!(
            guide.len() <= SUPERVISOR_GUIDANCE_SOFT_CAP_BYTES,
            "supervisor_guidance is {} bytes — over the {SUPERVISOR_GUIDANCE_SOFT_CAP_BYTES}B soft cap (only \
             {}B from the {SUPERVISOR_GUIDANCE_HARD_CEILING_BYTES}B hard ceiling). Trim body prose or move it \
             into cas-supervisor/references/ to keep CI headroom.",
            guide.len(),
            SUPERVISOR_GUIDANCE_HARD_CEILING_BYTES - guide.len()
        );
    }

    #[test]
    fn test_worker_guidance_loads() {
        let guide = worker_guidance();
        assert!(guide.contains("Worker"));
        assert!(!guide.contains("managed_by:"));
        // Worker should NOT have supervisor checklist
        assert!(
            !guide.contains("Supervisor Checklist"),
            "should not include supervisor checklist"
        );
        // task-tracking/memory/search are autonomous skills, not bundled.
        assert!(
            !guide.contains("Cassy Task Tracking"),
            "should NOT bundle task-tracking — loads on demand"
        );
    }

    /// Same rationale as `test_supervisor_guidance_under_12kb` — the worker
    /// SessionStart bundle must stay small enough that the harness doesn't
    /// truncate it to a preview. Move content into cas-worker/references/
    /// instead of inlining.
    #[test]
    fn test_worker_guidance_under_12kb() {
        const HARD_CEILING: usize = 12_288; // Claude Code harness truncation point.
        const SOFT_CAP: usize = 11_488; // early-warning margin (800B) below the ceiling.
        let guide = worker_guidance();
        assert!(
            guide.len() < HARD_CEILING,
            "worker_guidance is {} bytes — over the 12KB ceiling. \
             Move content into cas-worker/references/ instead of \
             inlining it in cas-worker.md.",
            guide.len()
        );
        assert!(
            guide.len() <= SOFT_CAP,
            "worker_guidance is {} bytes — over the {SOFT_CAP}B soft cap (only \
             {}B from the {HARD_CEILING}B hard ceiling). Trim body prose or move it \
             into cas-worker/references/ to keep CI headroom.",
            guide.len(),
            HARD_CEILING - guide.len()
        );
    }

    /// cas-5787 (EPIC cas-ebea, third-brain borrow): both supervisor and
    /// worker skill bodies must document the "Context budgeting" 3-layer
    /// model so future maintainers see the framework before adding to the
    /// Immutable Core (this skill body). The section names the three
    /// layers explicitly (Immutable Core / Task Context / Ephemeral),
    /// cites its component ceiling (8 KB supervisor / 12 KB worker) plus,
    /// for the worker, the 9 KB aggregate SessionStart budget, and points
    /// at the rationale memory file
    /// `project_session_start_truncation.md`. Both Claude and Codex
    /// mirrors are checked so neither surface silently drifts.
    #[test]
    fn test_skills_document_context_budgeting_cas_5787() {
        // Common markers required in all four skill files.
        let common = [
            "## Context budgeting",
            "Immutable Core",
            "Task Context",
            "Ephemeral",
            "project_session_start_truncation.md",
            "references/",
        ];
        // Supervisor cap was lowered to 8KB (cas-5e4b); the worker *component*
        // cap remains 12KB. cas-4c25: the worker body must additionally name
        // the 9KB aggregate SessionStart budget introduced by cas-b114, so the
        // section can't drift back to describing a silent-truncation model
        // that no longer exists.
        // cas-703a: grok was previously absent from this list, so the grok
        // bodies could lose a context-budgeting marker with this test green.
        // Grok is now checked here directly, and additionally guarded by
        // cas-cli/tests/builtin_flavor_drift_test.rs, which holds all three
        // flavors of these two files content-identical after normalization.
        let supervisor_files = [
            ("claude cas-supervisor.md", SUPERVISOR_GUIDE),
            (
                "codex cas-supervisor.md",
                include_str!("builtins/codex/skills/cas-supervisor.md"),
            ),
            (
                "grok cas-supervisor.md",
                include_str!("builtins/grok/skills/cas-supervisor.md"),
            ),
        ];
        let worker_files = [
            ("claude cas-worker.md", WORKER_GUIDE),
            (
                "codex cas-worker.md",
                include_str!("builtins/codex/skills/cas-worker.md"),
            ),
            (
                "grok cas-worker.md",
                include_str!("builtins/grok/skills/cas-worker.md"),
            ),
        ];
        for (label, content) in supervisor_files {
            for required in common.iter().chain(["8 KB"].iter()) {
                assert!(
                    content.contains(required),
                    "{label} missing required Context-budgeting marker: {required:?}"
                );
            }
        }
        for (label, content) in worker_files {
            for required in common.iter().chain(["12 KB", "9 KB"].iter()) {
                assert!(
                    content.contains(required),
                    "{label} missing required Context-budgeting marker: {required:?}"
                );
            }
        }
    }

    // cas-5be8: disallowed-tools frontmatter in builtin skills
    #[test]
    fn test_builtin_cas_worker_disallowed_tools() {
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-worker/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: cas-worker SKILL.md missing"));
            for required in ["disallowed-tools:", "- TodoWrite", "- EnterPlanMode"] {
                assert!(
                    entry.content.contains(required),
                    "{label}: cas-worker SKILL.md missing disallowed-tools entry: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_report_evidence_guidance_prefers_safe_sources() {
        for (label, skill_content, details_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-worker.md"),
                include_str!("builtins/skills/cas-worker/references/details.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-worker.md"),
                include_str!("builtins/codex/skills/cas-worker/references/details.md"),
            ),
        ] {
            for required in [
                "Report / evidence tasks",
                "MCP task/search/coordination surfaces",
                ".cas/logs",
                "read-only SQLite URI",
                "copied snapshot",
            ] {
                assert!(
                    skill_content.contains(required) || details_content.contains(required),
                    "{label} worker guidance missing report/evidence safety marker: {required:?}"
                );
            }
            assert!(
                details_content
                    .contains("Do **not** use unrestricted `sqlite3 /path/to/.cas/cas.db`"),
                "{label} worker details should explicitly discourage unrestricted live sqlite3 access"
            );
        }

        for (label, planning_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-supervisor/references/planning.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-supervisor/references/planning.md"),
            ),
        ] {
            for required in [
                "Evidence-source plan",
                "MCP/log/recording/task-record sources",
                ".cas/cas.db",
                "read-only URI or copied-snapshot access",
            ] {
                assert!(
                    planning_content.contains(required),
                    "{label} supervisor planning guidance missing report/evidence template marker: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_builtin_cas_brainstorm_disallowed_tools() {
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-brainstorm/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: cas-brainstorm SKILL.md missing"));
            for required in ["disallowed-tools:", "- Write", "- Edit", "- NotebookEdit"] {
                assert!(
                    entry.content.contains(required),
                    "{label}: cas-brainstorm SKILL.md missing disallowed-tools entry: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_builtin_cas_ideate_disallowed_tools() {
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-ideate/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: cas-ideate SKILL.md missing"));
            for required in ["disallowed-tools:", "- Write", "- Edit", "- NotebookEdit"] {
                assert!(
                    entry.content.contains(required),
                    "{label}: cas-ideate SKILL.md missing disallowed-tools entry: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_is_managed_by_cas() {
        let managed = "---\nname: test\nmanaged_by: cas\n---\nContent";
        assert!(is_managed_by_cas(managed));

        let not_managed = "---\nname: test\n---\nContent";
        assert!(!is_managed_by_cas(not_managed));

        let no_frontmatter = "# Just content";
        assert!(!is_managed_by_cas(no_frontmatter));
    }

    #[test]
    fn test_builtin_agents_contains_git_history_analyzer() {
        assert!(
            BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/git-history-analyzer.md")
        );
        assert!(
            CODEX_BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/git-history-analyzer.md")
        );
    }

    #[test]
    fn test_builtin_agents_contains_issue_intelligence_analyst() {
        assert!(
            BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/issue-intelligence-analyst.md")
        );
        assert!(
            CODEX_BUILTIN_AGENTS
                .iter()
                .any(|b| b.path == "agents/issue-intelligence-analyst.md")
        );
    }

    #[test]
    fn test_builtin_skills_contains_cas_brainstorm() {
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/SKILL.md")
        );
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/references/handoff.md")
        );
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/references/requirements-capture.md")
        );
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-brainstorm/SKILL.md")
        );
    }

    #[test]
    fn test_builtin_skills_contains_cas_ideate() {
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-ideate/SKILL.md")
        );
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-ideate/references/post-ideation-workflow.md")
        );
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/cas-ideate/SKILL.md")
        );
    }

    /// cas-b4921 / GH #121: the two ways a worker dies expensively, both
    /// preventable and both guidance gaps.
    ///
    /// PART A — a worker that foreground-blocks on a long command is
    /// unreachable: supervisor messages, stand-down orders and urgent stops are
    /// only delivered between turns. One worker burned 40+ minutes
    /// foreground-watching queued CI through a provider outage, ignoring nine
    /// messages including two stand-down orders.
    ///
    /// PART B — both workers in that window ran into auto-compaction and were
    /// killed mid-compaction, so the operator paid to re-summarize work a
    /// `git push` would have preserved.
    ///
    /// All THREE harness flavors must carry both mandates: terse hard rules on
    /// the always-loaded surface (the SKILL.md body is on the hot path and has
    /// a 12 KB ceiling — see `test_worker_guidance_under_12kb`) with the
    /// recipes in references/discipline.md. The flavor set is spelled out here
    /// because this guidance has a documented drift history (GH #116).
    #[test]
    fn test_worker_skills_carry_backgrounding_mandate_cas_b4921() {
        for (label, skill_content, ref_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-worker.md"),
                include_str!("builtins/skills/cas-worker/references/discipline.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-worker.md"),
                include_str!("builtins/codex/skills/cas-worker/references/discipline.md"),
            ),
            (
                "grok",
                include_str!("builtins/grok/skills/cas-worker.md"),
                include_str!("builtins/grok/skills/cas-worker/references/discipline.md"),
            ),
        ] {
            // Always-loaded body: terse hard rules only (Rules of Engagement),
            // each naming its threshold, plus the breadcrumb to the recipes.
            for required in [
                "Never block the pane",
                "~2 minutes",
                "gh run watch",
                "action=remind",
                "Checkpoint, never compact",
                "discipline.md",
                "last_token_usage",
                "model_context_window",
                "total_token_usage",
            ] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing hard-rule marker: {required:?}"
                );
            }
            // Scoped-test discipline lives with the test guidance (A-3):
            // compile continuously without linking test binaries, then use
            // nextest only for the affected target. These are deliberately
            // concrete commands rather than a generic "be scoped" reminder.
            for required in [
                "cargo check -p <crate> --lib --tests",
                "cargo nextest run",
                "--lib <module>",
                "--test <name>",
            ] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing scoped-test marker: {required:?}"
                );
            }
            // Part A recipes: background builds, the server registry for
            // anything listening, the sanctioned CI shape (queue rerun ->
            // reminder -> end turn -> one-shot check), and inbox_poll first if
            // you did come back from a blocked turn.
            for required in [
                "run_in_background",
                "action=server_start",
                "remind_delay_secs",
                "gh run list",
                "inbox_poll",
            ] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker discipline.md missing backgrounding recipe: {required:?}"
                );
            }
            assert!(
                ref_content.contains("BANNED") || ref_content.contains("banned"),
                "{label} cas-worker discipline.md must explicitly ban foreground CI watching"
            );
            // Part B: headroom reporting, the four-step checkpoint protocol,
            // and commit sizing.
            for required in [
                "note_type=progress",
                "Context: ~",
                "CHECKPOINT",
                "git push",
                "respawn",
                "auto-compaction",
                "37,952",
                "258,400",
                "356,457",
            ] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker discipline.md missing context-budget marker: {required:?}"
                );
            }
        }
    }

    /// cas-641f: workers must walk the repository's cross-surface blast
    /// radius before close, rather than proving only the path they changed.
    #[test]
    fn test_worker_skills_require_cas_src_surface_checklist() {
        for (label, content) in [
            ("claude", include_str!("builtins/skills/cas-worker.md")),
            ("codex", include_str!("builtins/codex/skills/cas-worker.md")),
            ("grok", include_str!("builtins/grok/skills/cas-worker.md")),
        ] {
            for required in [
                "cas-src surface checklist",
                "This is a requirement, not a suggestion",
                "not applicable",
                "Codex, and Grok mirrors",
                "CLI parity, docs, and dispatch registration",
                "config_gen",
                ".codex/hooks.json",
                "bootstrap/reconciliation expectations",
                "doctor_snapshot",
                "cas-2327",
                "reverse states",
                "release-notes impact",
            ] {
                assert!(content.contains(required), "{label} surface checklist missing {required:?}");
            }
        }
    }

    /// cas-3627 (GH #159): the worker builtin must teach the difference
    /// between the INNER test loop and the FINAL proof.
    ///
    /// Observed live before this rule existed: a worker fixing several test
    /// entry points ran the full ~3,700-test lib sweep (~5 min) after each
    /// individual micro-fix, foreground-`sleep`ing between checks — 47+
    /// minutes of wall-clock for a few minutes of edits. cas-b4921 already
    /// mandated backgrounding, so the gap was not "don't block"; it was that
    /// nothing distinguished the seconds-long targeted loop you iterate in
    /// from the minutes-long full sweep you are allowed to run twice.
    ///
    /// Same two-layer shape as the backgrounding mandate above: terse rules on
    /// the always-loaded SKILL.md body (12 KB ceiling — see
    /// `test_worker_guidance_under_12kb`), the worked recipe in
    /// references/discipline.md. All three flavors, because the builtin skills
    /// have a documented drift history (GH #116, cas-703a).
    #[test]
    fn test_worker_skills_teach_test_loop_discipline_cas_3627() {
        for (label, skill_content, ref_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-worker.md"),
                include_str!("builtins/skills/cas-worker/references/discipline.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-worker.md"),
                include_str!("builtins/codex/skills/cas-worker/references/discipline.md"),
            ),
            (
                "grok",
                include_str!("builtins/grok/skills/cas-worker.md"),
                include_str!("builtins/grok/skills/cas-worker/references/discipline.md"),
            ),
        ] {
            // Always-loaded body: batch-first, the two named loops, the
            // two-sweep ceiling, nextest, and the no-foreground-sleep rule.
            for required in [
                "Batch the fixes",
                "Inner loop",
                "Final proof",
                "at most twice",
                "cargo nextest run",
                "sleep",
            ] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing test-loop marker: {required:?}"
                );
            }
            // The recipe: both loops named, the batching rule, the banked-receipt
            // allowance, nextest with a fallback, and the ban on sleeping in the
            // foreground while a background run cooks.
            for required in [
                "inner loop",
                "Final proof",
                "Batch before you verify",
                "banked receipt",
                "cargo nextest run",
                "foreground-`sleep`",
            ] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker discipline.md missing test-loop recipe: {required:?}"
                );
            }
            // The targeted-filter forms are the whole point of the inner loop:
            // a rule that says "be targeted" without naming the flags is not
            // actionable at 2am.
            for required in ["--lib <module>", "--test <name>"] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker discipline.md missing targeted-filter form: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_cas_worker_skill_documents_code_review_gate() {
        // Phase 1 Subsystem A Unit 10 (EPIC cas-0750): the cas-worker
        // skill must document the new close-time code-review gate so
        // workers know how to read the block message, what happens to
        // residual findings, and which tools they must NOT fall back
        // to. After the cas-61af split, SKILL.md keeps the high-signal
        // references (cas-code-review and the close-gate pointer) and
        // the detailed P0/bypass/legacy-tool guidance lives in
        // references/close-gate.md. Pin both layers structurally so
        // drift through cas sync cannot silently delete them.
        for (label, skill_content, ref_content) in [
            (
                "claude",
                include_str!("builtins/skills/cas-worker.md"),
                include_str!("builtins/skills/cas-worker/references/close-gate.md"),
            ),
            (
                "codex",
                include_str!("builtins/codex/skills/cas-worker.md"),
                include_str!("builtins/codex/skills/cas-worker/references/close-gate.md"),
            ),
        ] {
            // SKILL.md points workers at the gate (via close-gate.md).
            //
            // Historical note (cas-ec8f amendment): this loop previously also
            // asserted the literal substring "cas-code-review" was present in
            // cas-worker.md, but commit 8b82273 / cas-8962 deliberately
            // removed that mention when `[code_review] owner = "supervisor"`
            // became the default (v2.13.0+). Workers must NOT invoke
            // cas-code-review pre-close under the default ownership model —
            // the supervisor owns review timing, with a lightweight
            // per-merge gate and one full review at EPIC code-complete.
            // The assertion was silently failing on main from that commit
            // forward; cas-ec8f drops it here so the test reflects the
            // current ownership contract. The `close-gate.md` pointer is
            // still required — that doc is where the detailed gate content
            // lives and workers do need to know about it.
            for required in ["close-gate.md"] {
                assert!(
                    skill_content.contains(required),
                    "{label} cas-worker SKILL.md missing required marker: {required:?}"
                );
            }
            // close-gate.md carries the detailed gate content.
            //
            // Historical note (cas-ec8f amendment): this list previously
            // pinned five markers that documented the legacy worker-inline
            // code-review path: "Close-time Code Review Gate" (old section
            // title), "If close is blocked on P0" (legacy P0 hard-block
            // behavior), "bypass_code_review" (legacy worker bypass), plus
            // "cas-code-review" and "code-reviewer". Commit 167c57e
            // ("docs(skills): finish cas-5815 supervisor-default flip —
            // purge stale worker-runs-review prompts") deliberately rewrote
            // close-gate.md when `[code_review] owner = "supervisor"` became
            // the default — the inline-block markers no longer apply.
            // The assertions were silently failing on main from that point
            // forward. The new pin set encodes the *current* ownership
            // contract: close-gate.md documents the close gate, points
            // workers at cas-code-review with a "don't invoke pre-close"
            // caveat, and names the supervisor-owned default ownership flag.
            for required in ["Close Gate", "cas-code-review", "owner = \"supervisor\""] {
                assert!(
                    ref_content.contains(required),
                    "{label} cas-worker close-gate.md missing required marker: {required:?}"
                );
            }
        }
    }

    /// cas-7c93 (GH #87): the cas-servers skill must ship for every harness
    /// and must keep the content that makes it load-bearing — the anti-pattern
    /// it replaces, the survival rule that is the whole reason to register,
    /// and the three actions.
    ///
    /// The three copies are identical *modulo the harness tool prefix*: each
    /// CLI resolves a different one (`mcp__cas__` / `mcp__cs__` / `cas__`), so
    /// a byte-identical mirror would hand two of the three harnesses tool
    /// names that do not resolve.
    #[test]
    fn test_builtin_skills_contains_cas_servers() {
        let mut bodies = Vec::new();
        for (label, catalog, prefix) in [
            ("claude", BUILTIN_SKILLS, "mcp__cas__"),
            ("codex", CODEX_BUILTIN_SKILLS, "mcp__cs__"),
            ("grok", GROK_BUILTIN_SKILLS, "cas__"),
        ] {
            let entry = catalog
                .iter()
                .find(|b| b.path == "skills/cas-servers/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("skills/cas-servers/SKILL.md missing from {label} catalog")
                });
            assert!(
                is_managed_by_cas(entry.content),
                "{label} cas-servers SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: cas-servers",
                // The action surface.
                "action=server_start",
                "action=server_stop",
                "action=server_list",
                // The anti-pattern this skill exists to replace, named
                // explicitly so an agent recognizes what it is doing wrong.
                "npm run dev &",
                "Never background a server yourself",
                // The load-bearing rule: registration IS survival.
                "Registered servers are the only ones that survive worker teardown",
                // Attribution is what makes server_list answer "who started it".
                "task_id",
                // Shared vs private, and who owns the cleanup.
                "shared=true",
                "you are responsible for stopping it",
                // Scope guard: one-shot commands do not belong in the registry.
                "One-shot commands do not belong here",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label} cas-servers SKILL.md missing required marker: {required:?}"
                );
            }
            // The tool calls must be spelled the way this harness resolves them.
            assert!(
                entry.content.contains(&format!("{prefix}coordination")),
                "{label} cas-servers SKILL.md must call {prefix}coordination"
            );
            bodies.push((label, entry.content.replace(prefix, "<PREFIX>")));
        }

        let claude_body = bodies[0].1.clone();
        for (label, body) in &bodies[1..] {
            assert_eq!(
                *body, claude_body,
                "{label} cas-servers SKILL.md must match the claude copy except for the \
                 harness tool prefix — the guidance itself must not drift per harness"
            );
        }
    }

    /// cas-1219: MCP installation guidance is intentionally the same field
    /// runbook for every harness. Its diagnosis reference is part of the
    /// skill's contract, not an optional local file, so sync must deliver both
    /// files for Claude, Codex, and Grok without any content drift.
    #[test]
    fn test_builtin_skills_contains_mcp_integration() {
        use tempfile::tempdir;

        const SKILL: &str = "skills/mcp-integration/SKILL.md";
        const DIAGNOSIS: &str = "skills/mcp-integration/references/diagnosis.md";
        let mut shipped = Vec::new();

        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let skill = catalog
                .iter()
                .find(|entry| entry.path == SKILL)
                .unwrap_or_else(|| panic!("{SKILL} missing from {label} catalog"));
            assert!(
                is_managed_by_cas(skill.content),
                "{label} {SKILL} must be managed_by: cas"
            );
            assert!(
                skill.content.contains("name: mcp-integration"),
                "{label} {SKILL} must retain its skill name"
            );
            assert!(
                skill
                    .content
                    .contains("[references/diagnosis.md](references/diagnosis.md)"),
                "{label} {SKILL} must retain the shipped diagnosis link"
            );

            let diagnosis = catalog
                .iter()
                .find(|entry| entry.path == DIAGNOSIS)
                .unwrap_or_else(|| panic!("{DIAGNOSIS} missing from {label} catalog"));
            assert!(
                diagnosis.content.contains("# MCP diagnosis reference"),
                "{label} {DIAGNOSIS} must retain the diagnosis reference"
            );

            shipped.push((label, skill.content, diagnosis.content));
        }

        let (_, claude_skill, claude_diagnosis) = shipped[0];
        for (label, skill, diagnosis) in &shipped[1..] {
            assert_eq!(
                *skill, claude_skill,
                "{label} {SKILL} must be byte-identical to the Claude copy"
            );
            assert_eq!(
                *diagnosis, claude_diagnosis,
                "{label} {DIAGNOSIS} must be byte-identical to the Claude copy"
            );
        }

        let temp = tempdir().unwrap();
        for (label, sync) in [
            (
                "claude",
                sync_all_builtins as fn(&Path) -> std::io::Result<SyncResult>,
            ),
            (
                "codex",
                sync_all_codex_builtins as fn(&Path) -> std::io::Result<SyncResult>,
            ),
            (
                "grok",
                sync_all_grok_builtins as fn(&Path) -> std::io::Result<SyncResult>,
            ),
        ] {
            let target = temp.path().join(label);
            sync(&target).unwrap();
            assert_eq!(
                std::fs::read_to_string(target.join(SKILL)).unwrap(),
                claude_skill
            );
            assert_eq!(
                std::fs::read_to_string(target.join(DIAGNOSIS)).unwrap(),
                claude_diagnosis
            );
        }
    }

    /// cas-b176: the cas-html-reports skill must ship for every harness with its
    /// full reference set. The load-bearing content is the two-axis taxonomy
    /// (type x audience), the invariant technical contract, the judgment rules
    /// that keep chat answers out of HTML, and the two worked examples. The
    /// examples are also asserted to be genuinely self-contained — an example
    /// that reaches for a CDN teaches the opposite of the contract it documents.
    #[test]
    fn test_builtin_skills_contains_cas_html_reports() {
        const FILES: &[&str] = &[
            "skills/cas-html-reports/SKILL.md",
            "skills/cas-html-reports/references/report-types.md",
            "skills/cas-html-reports/references/presentation-rules.md",
            "skills/cas-html-reports/references/technical-contract.md",
            "skills/cas-html-reports/references/review-checklist.md",
            "skills/cas-html-reports/references/sources.md",
            "skills/cas-html-reports/references/examples/engineering-investigation.html",
            "skills/cas-html-reports/references/examples/financial-quarterly-brief.html",
        ];

        let mut claude_bodies: Vec<(&str, &str)> = Vec::new();

        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let get = |path: &str| -> &'static str {
                catalog
                    .iter()
                    .find(|b| b.path == path)
                    .unwrap_or_else(|| panic!("{path} missing from {label} catalog"))
                    .content
            };

            let skill = get(FILES[0]);
            assert!(
                is_managed_by_cas(skill),
                "{label} cas-html-reports SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: cas-html-reports",
                // The core stance the whole skill hangs off.
                "Markdown is the source of truth",
                // Trigger + the judgment rule that prevents HTML overuse.
                "What counts as a report",
                "When HTML is NOT required",
                "Task notes",
                // The invariant contract, summarized on the front page.
                "No CDN",
                "progressive enhancement",
                "Print-ready",
                "Provenance per figure",
                // Both worked examples must be advertised, not just shipped.
                "engineering-investigation.html",
                "financial-quarterly-brief.html",
            ] {
                assert!(
                    skill.contains(required),
                    "{label} cas-html-reports SKILL.md missing required marker: {required:?}"
                );
            }

            // The two-axis taxonomy is the acceptance surface: every report type
            // AND the audience axis must be reachable from one reference file.
            let types = get(FILES[1]);
            for required in [
                "Executive",
                "Practitioner",
                "External",
                "Investigation / diagnostic",
                "Metrics / mining analysis",
                "Decision brief",
                "Comparison / benchmark",
                "Incident / post-mortem",
                "Status / release summary",
                "Financial report",
                "Executive / C-suite brief",
                "Board / stakeholder update",
                "Client-facing deliverable",
                "Research / market analysis",
                // Financial encodings (IBCS-derived) must be spelled out, not implied.
                "Actual** = solid fill",
                "outlined",
                "hatched",
                // Executive ordering rule.
                "methodology is present but LAST",
            ] {
                assert!(
                    types.contains(required),
                    "{label} cas-html-reports report-types.md missing marker: {required:?}"
                );
            }

            // Attribution-only citation of all three research sources.
            let sources = get(FILES[5]);
            for required in [
                "html-artifact-best-practices",
                "IBCS",
                "pi-skill-html-report",
                "attribution only",
            ] {
                assert!(
                    sources.contains(required),
                    "{label} cas-html-reports sources.md missing marker: {required:?}"
                );
            }

            // The examples must practice what the contract preaches.
            for example in &FILES[6..] {
                let html = get(example);
                for banned in ["https://", "http://", "@import", "<img", "cdn."] {
                    assert!(
                        !html.to_lowercase().contains(banned),
                        "{label} {example} must be self-contained (found {banned:?})"
                    );
                }
                for required in ["<!DOCTYPE html>", "@media print", "role=\"img\"", "<table"] {
                    assert!(
                        html.contains(required),
                        "{label} {example} missing required element: {required:?}"
                    );
                }
            }

            // The skill makes no CAS MCP tool calls, so the twins are held
            // byte-identical — no per-harness prefix to swap.
            if label == "claude" {
                claude_bodies = FILES.iter().map(|f| (*f, get(f))).collect();
            } else {
                for (path, claude) in &claude_bodies {
                    assert_eq!(
                        get(path),
                        *claude,
                        "{label} {path} must be byte-identical to the claude copy \
                         (cas-html-reports is harness-neutral)"
                    );
                }
            }
        }
    }

    /// cas-1e7e: visualization guidance is a cross-harness skill rather than
    /// a Claude-only bundled capability. Keep its trigger surface, review,
    /// validator, and static evidence example together in every catalog.
    #[test]
    fn test_builtin_skills_contains_cas_dataviz() {
        const FILES: &[&str] = &[
            "skills/cas-dataviz/SKILL.md",
            "skills/cas-dataviz/references/design-review.md",
            "skills/cas-dataviz/references/quality-checklist.md",
            "skills/cas-dataviz/scripts/validate_palette.js",
            "skills/cas-dataviz/examples/2026-08-11-commit-classes.html",
        ];
        let mut claude_bodies = Vec::new();
        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let get = |path: &str| -> &'static str {
                catalog.iter().find(|b| b.path == path)
                    .unwrap_or_else(|| panic!("{path} missing from {label} catalog")).content
            };
            let skill = get(FILES[0]);
            assert!(is_managed_by_cas(skill), "{label} cas-dataviz must be managed by Cassy");
            let description = skill.lines().find_map(|line| line.strip_prefix("description: "))
                .expect("cas-dataviz needs a description");
            assert!(description.len() <= 360, "{label} trigger description exceeds 360 bytes");
            for marker in [
                "message, not the chart", "claim-title", "annotate the decisive", "Show uncertainty",
                "small multiples", "table", "@media print", "cas-html-reports", "color last",
                "becoming text-dense", "30 seconds", "Visually verify the rendered artifact",
                "390×844", "Grepping HTML", "H7 acceptance-report precedent",
            ] {
                assert!(skill.contains(marker), "{label} cas-dataviz missing {marker:?}");
            }
            let review = get(FILES[1]);
            for marker in ["Preserve", "Missing", "Deliberate default inversions", "color-last procedure", "computable palette validator", "one-axis rule", "message-first", "annotation practice", "print/PDF"] {
                assert!(review.contains(marker), "{label} design review missing {marker:?}");
            }
            assert!(get(FILES[3]).contains("export function validate"), "{label} missing runnable validator");
            let example = get(FILES[4]);
            for marker in ["<!DOCTYPE html>", "@media print", "role=\"img\"", "<table", "Provenance:", "Merge commits were the largest"] {
                assert!(example.contains(marker), "{label} example missing {marker:?}");
            }
            if label == "claude" {
                claude_bodies = FILES.iter().map(|path| (*path, get(path))).collect();
            } else {
                for (path, claude) in &claude_bodies {
                    assert_eq!(get(path), *claude, "{label} {path} must match the Claude mirror");
                }
            }
        }
    }

    /// cas-ff2f (GH #94): the cas-github-issues sweep skill must ship for every
    /// harness — the hourly cron prompt invokes it by name, and it previously
    /// resolved to nothing. The body must cover all six sweep steps, including
    /// the no-active-epic branch that is the easiest one to get wrong.
    #[test]
    fn test_builtin_skills_contains_cas_github_issues() {
        let mut bodies = Vec::new();
        for (label, catalog, prefix) in [
            ("claude", BUILTIN_SKILLS, "mcp__cas__"),
            ("codex", CODEX_BUILTIN_SKILLS, "mcp__cs__"),
            ("grok", GROK_BUILTIN_SKILLS, "cas__"),
        ] {
            let entry = catalog
                .iter()
                .find(|b| b.path == "skills/cas-github-issues/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("skills/cas-github-issues/SKILL.md missing from {label} catalog")
                });
            assert!(
                is_managed_by_cas(entry.content),
                "{label} cas-github-issues SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: cas-github-issues",
                // Step 1 — enumerate the open issues.
                "gh issue list --state open",
                // Step 2 — dedupe double-filings (the multi-machine failure mode).
                "Dedupe double-filings",
                "Duplicate of #",
                // Step 3 — a "fixed" claim is a claim, not a fact.
                "Verify-and-close fixed claims",
                "Verify against the code",
                // Step 4 — task into the ACTIVE epic, creating a successor when
                // none is open. This is the branch that was exercised for real
                // and the one an agent gets wrong by default.
                "Every epic for this lane is closed",
                "create a successor epic first",
                "Never task into a closed epic",
                // The status filter is a substring match, so `status=open`
                // hides every task/epic somebody is actually working on. A
                // sweep that filters that way invents a duplicate successor
                // epic while the real one is mid-flight.
                "Do not filter this list by `status=open`",
                "auto-promoted to `in_progress`",
                "action=create",
                "external_ref",
                "gh issue comment",
                // Step 5 — unblock chained tasks, and only on MERGED blockers.
                "action=blocked",
                "action=dep_remove",
                "Merged is the bar, not closed",
                // Step 6 — file what you observed since the last sweep, using
                // the same six-heading body every other issue in the tracker
                // uses.
                "gh issue create",
                "**Environment**",
                "**Repro**",
                "**Actual**",
                "**Expected**",
                "**Impact**",
                "**Suggested fix**",
                // The cron contract: the entry expires, and an expired sweep
                // is indistinguishable from a clean one.
                ".claude/scheduled_tasks.json",
                "7-day auto-expiry",
                "recreate it if it expired",
                // The sweep's success case is silence.
                "end the turn silently",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label} cas-github-issues SKILL.md missing required marker: {required:?}"
                );
            }
            assert!(
                entry.content.contains(&format!("{prefix}task")),
                "{label} cas-github-issues SKILL.md must call {prefix}task"
            );
            bodies.push((label, entry.content.replace(prefix, "<PREFIX>")));
        }

        let claude_body = bodies[0].1.clone();
        for (label, body) in &bodies[1..] {
            assert_eq!(
                *body, claude_body,
                "{label} cas-github-issues SKILL.md must match the claude copy except for \
                 the harness tool prefix — the sweep procedure must not drift per harness"
            );
        }
    }

    /// GH #64: the design-spec skill (DESIGN.md generator) must be registered
    /// for every harness so `cas sync` installs it, and its body must keep the
    /// contract that makes it useful: live-token grounding, the fixed 8
    /// sections, keep-block preservation, and the memory pointer.
    #[test]
    fn test_builtin_skills_contains_design_spec() {
        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let entry = catalog
                .iter()
                .find(|b| b.path == "skills/design-spec/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("skills/design-spec/SKILL.md missing from {label} catalog")
                });
            assert!(
                is_managed_by_cas(entry.content),
                "{label} design-spec SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: design-spec",
                "DESIGN.md",
                // Token source is authoritative, not any prose design doc.
                "The code is the source of truth",
                // The fixed 8-section output contract.
                "## Overview",
                "## Colors",
                "## Typography",
                "## Layout",
                "## Elevation & Depth",
                "## Shapes",
                "## Components",
                "## Do's & Don'ts",
                // Regeneration must not destroy hand edits.
                "<!-- keep -->",
                // Thin pointer so Cassy search surfaces the doc.
                "project_<slug>_designmd",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label} design-spec SKILL.md missing required marker: {required:?}"
                );
            }
        }
    }

    /// GH #65: the release-notes skill plus its canonical rubric template must
    /// ship for every harness, and both must keep the hard rules (Was → Now,
    /// no ticket IDs, no process talk, two threads with one reply each).
    #[test]
    fn test_builtin_skills_contains_release_notes_rubric() {
        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let skill = catalog
                .iter()
                .find(|b| b.path == "skills/release-notes/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("skills/release-notes/SKILL.md missing from {label} catalog")
                });
            assert!(
                is_managed_by_cas(skill.content),
                "{label} release-notes SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: release-notes",
                "docs/release-notes/RUBRIC.md",
                "references/RUBRIC-template.md",
                "Was → Now",
                "Live on production",
                "Staging",
                "## POSTED",
                "UTC timestamp",
            ] {
                assert!(
                    skill.content.contains(required),
                    "{label} release-notes SKILL.md missing required marker: {required:?}"
                );
            }

            let template = catalog
                .iter()
                .find(|b| b.path == "skills/release-notes/references/RUBRIC-template.md")
                .unwrap_or_else(|| {
                    panic!(
                        "skills/release-notes/references/RUBRIC-template.md missing from \
                         {label} catalog"
                    )
                });
            for required in [
                "Was → Now",
                "no internal ticket labels",
                "docs/release-notes/<date>-<topic>-slack.md",
                "## POSTED",
                "UTC timestamp",
            ] {
                assert!(
                    template.content.to_lowercase().contains(&required.to_lowercase()),
                    "{label} RUBRIC-template.md missing required rule: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_builtin_skills_contains_project_overview() {
        // EPIC cas-19a2b: project-overview SKILL.md must be registered so
        // `cas sync` installs it at .claude/skills/project-overview/SKILL.md.
        assert!(
            BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/project-overview/SKILL.md"),
            "skills/project-overview/SKILL.md missing from BUILTIN_SKILLS"
        );
        assert!(
            CODEX_BUILTIN_SKILLS
                .iter()
                .any(|b| b.path == "skills/project-overview/SKILL.md"),
            "skills/project-overview/SKILL.md missing from CODEX_BUILTIN_SKILLS"
        );

        // Content sanity: frontmatter trigger phrases + required post-write
        // steps (memory pointer + freshness clear) must survive any drift.
        let entry = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/project-overview/SKILL.md")
            .unwrap();
        for required in [
            "name: project-overview",
            "managed_by: cas",
            "docs/PRODUCT_OVERVIEW.md",
            "<!-- keep -->",
            "mcp__cas__memory",
            "cas project-overview clear",
        ] {
            assert!(
                entry.content.contains(required),
                "project-overview SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_fallow() {
        // Vendored from fallow-rs/fallow-skills (MIT). SKILL.md plus three
        // references must be registered in both Claude and Codex mirrors so
        // `cas sync` installs the full skill.
        let expected = [
            "skills/fallow/SKILL.md",
            "skills/fallow/references/cli-reference.md",
            "skills/fallow/references/gotchas.md",
            "skills/fallow/references/patterns.md",
        ];
        for p in expected {
            assert!(
                BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from BUILTIN_SKILLS"
            );
            assert!(
                CODEX_BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from CODEX_BUILTIN_SKILLS"
            );
        }

        // Frontmatter sanity: `managed_by: cas` is the marker that lets
        // `cas sync` overwrite stale downstream copies, and the upstream
        // attribution must survive any drift from the vendor's repo.
        let entry = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/fallow/SKILL.md")
            .unwrap();
        for required in [
            "name: fallow",
            "managed_by: cas",
            "license: MIT",
            "author: Bart Waardenburg",
            "upstream: https://github.com/fallow-rs/fallow-skills",
        ] {
            assert!(
                entry.content.contains(required),
                "fallow SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_cas_code_review() {
        // Phase 1 subsystem A (EPIC cas-0750): 9 files per mirror; the
        // `fallow` persona added later brings the count to 10.
        let expected = [
            "skills/cas-code-review/SKILL.md",
            "skills/cas-code-review/references/findings-schema.md",
            "skills/cas-code-review/references/personas/correctness.md",
            "skills/cas-code-review/references/personas/testing.md",
            "skills/cas-code-review/references/personas/maintainability.md",
            "skills/cas-code-review/references/personas/project-standards.md",
            "skills/cas-code-review/references/personas/fallow.md",
            "skills/cas-code-review/references/personas/security.md",
            "skills/cas-code-review/references/personas/performance.md",
            "skills/cas-code-review/references/personas/adversarial.md",
        ];
        for p in expected {
            assert!(
                BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from BUILTIN_SKILLS"
            );
            assert!(
                CODEX_BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from CODEX_BUILTIN_SKILLS"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_cas_codex_exec() {
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-codex-exec/SKILL.md")
            .expect("BUILTIN_SKILLS missing cas-codex-exec SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-codex-exec/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing cas-codex-exec SKILL.md");
        assert_eq!(
            claude.content, codex.content,
            "cas-codex-exec SKILL.md .claude and .codex copies must be byte-identical",
        );
        for required in [
            "name: cas-codex-exec",
            "managed_by: cas",
            "token-heavy READ-ONLY investigation",
            "codex exec -s read-only -m gpt-5.5",
            "If you find nothing, say so explicitly and name what you inspected.",
            "If `codex` is not installed",
        ] {
            assert!(
                claude.content.contains(required),
                "cas-codex-exec SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_cli_routing() {
        for (label, catalog) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let skill = catalog
                .iter()
                .find(|b| b.path == "skills/cli-routing/SKILL.md")
                .unwrap_or_else(|| {
                    panic!("skills/cli-routing/SKILL.md missing from {label} catalog")
                });
            assert!(
                is_managed_by_cas(skill.content),
                "{label} cli-routing SKILL.md must be managed_by: cas"
            );
            for required in [
                "name: cli-routing",
                "codex exec",
                "claude auth status --json",
                "pippenz@gmail.com",
                "unapproved account",
                "docs/SLACK_POSTING_RUNBOOK.md",
                "release-notes",
            ] {
                assert!(
                    skill.content.contains(required),
                    "{label} cli-routing SKILL.md missing required marker: {required:?}"
                );
            }
            assert!(
                !skill.content.contains("daniel@petrastella.io"),
                "{label} cli-routing SKILL.md retains the stale Daniel-only account gate"
            );
        }
    }

    /// Extract the `description:` value from a SKILL.md frontmatter block.
    /// Cassy skill descriptions are single-line YAML scalars (long, but a
    /// single physical line terminated by `\n`). Panics if the field is
    /// missing — every builtin SKILL.md is required to have one.
    #[cfg(test)]
    fn skill_description(content: &str) -> &str {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("description:") {
                return rest.trim_start();
            }
        }
        panic!("SKILL.md frontmatter missing required `description:` field");
    }

    #[test]
    fn test_cas_code_review_description_reflects_supervisor_owned_default() {
        // Regression for cas-ec8f. The skill's frontmatter description is
        // the FIRST thing the LLM sees when listing skills — when it
        // disagrees with the body, the description wins in practice. The
        // prior framing said "the pre-close quality gate for Cassy factory
        // workers" and called `autofix` at `task.close` "the primary
        // path", which caused workers to self-dispatch personas at close
        // even under the v2.13.0+ default `[code_review] owner =
        // "supervisor"` (~100K input tokens burned per close, observed on
        // solid-cobra-88 cas-219d session log + reproduced on
        // daring-swan-93 cas-f645 in the same session this test was
        // added in).
        //
        // The new framing must: (a) not call autofix "the primary path";
        // (b) not describe this as a worker pre-close gate without the
        // supervisor-owned caveat; (c) explicitly name the supervisor as
        // the owner under the default model. Both BUILTIN_SKILLS (.claude
        // surface) and CODEX_BUILTIN_SKILLS (.codex surface) must agree
        // — the two are sync-mirrored by `cas update` and any drift
        // resurfaces the original bug on whichever harness reads stale
        // copy.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-code-review/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: skills/cas-code-review/SKILL.md missing"));
            let description = skill_description(entry.content);

            // (a) `autofix` must not be framed as "the primary path".
            // The prior phrasing was literally "in `autofix` mode (the
            // primary path)" — we forbid the co-occurrence of those two
            // substrings, which is tight enough that any reasonable
            // phrasing that still framed autofix as primary would fail.
            assert!(
                !(description.contains("autofix") && description.contains("primary path")),
                "{label}: cas-code-review description still frames `autofix` as 'the primary path'. \
                 Under owner=\"supervisor\" (default since v2.13.0) the primary path is supervisor-driven \
                 review cadence, not worker close-time autofix. Description: {description:?}",
            );

            // (b) "pre-close quality gate" is the other stale framing.
            // Allow the substring only if the description also names
            // the supervisor — i.e. only with proper context.
            let mentions_pre_close = description.contains("pre-close quality gate");
            let mentions_supervisor = description.contains("supervisor");
            assert!(
                !mentions_pre_close || mentions_supervisor,
                "{label}: cas-code-review description says 'pre-close quality gate' without naming \
                 the supervisor — workers will read it as a directive to self-dispatch personas at \
                 task.close. Description: {description:?}",
            );

            // (c) The description must affirmatively name supervisor
            // ownership. Without this, the absence of (a) and (b) is
            // not enough — a stripped-down description that just says
            // "code review orchestrator" still leaves workers free to
            // invoke it pre-close by default.
            assert!(
                mentions_supervisor,
                "{label}: cas-code-review description must explicitly name the supervisor as the \
                 default invoker so workers do not self-dispatch personas at task.close. \
                 Description: {description:?}",
            );
        }
    }

    #[test]
    fn test_builtin_skills_contains_session_learn() {
        // cas-39f5: session-learn must be registered in both surfaces so
        // `cas update` installs it at .claude/skills/session-learn/SKILL.md
        // (and the .codex equivalent). Without this entry the SKILL.md
        // source file exists on disk but never reaches downstream caches.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            assert!(
                skills
                    .iter()
                    .any(|b| b.path == "skills/session-learn/SKILL.md"),
                "{label} missing session-learn SKILL.md registration"
            );
        }
    }

    #[test]
    fn test_session_learn_skill_covers_seven_signal_taxonomy() {
        // cas-39f5 AC: the skill body documents the 7-signal taxonomy
        // (concept, entity, correction, pattern, idea, decision, gap)
        // with each signal mapped to a Cassy entry_type. The taxonomy is the
        // contract the Rust handler will encode in v2 — if a signal name
        // disappears from the skill body, the handler's JSON-schema parse
        // path silently drops findings of that type. Pin every signal name
        // so any drift triggers a compile-time test failure.
        for (label, skills) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/session-learn/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: session-learn SKILL.md not registered"));
            for signal in [
                "Concept",
                "Entity",
                "Correction",
                "Pattern",
                "Idea",
                "Decision",
                "Gap",
            ] {
                assert!(
                    entry.content.contains(&format!("**{signal}**")),
                    "{label}: session-learn SKILL.md missing signal marker **{signal}**"
                );
            }
            // Must also document the kill-switch flag so users can find it.
            assert!(
                entry.content.contains("session_learn_auto"),
                "{label}: session-learn SKILL.md must document the \
                 `session_learn_auto` kill-switch flag"
            );
            // And must record the in-process vs subprocess decision the
            // AC required.
            assert!(
                entry.content.contains("in-process"),
                "{label}: session-learn SKILL.md must document the \
                 in-process vs subprocess decision (cas-39f5 AC)"
            );
        }
    }

    #[test]
    fn test_session_learn_skill_md_mirrors_are_identical() {
        // cas-39f5: the .claude and .codex copies of session-learn/SKILL.md
        // are sync-mirrored by `cas update`. Drift between them silently
        // produces a different classifier prompt on whichever harness
        // reads the stale copy — exactly the failure mode cas-ec8f traced
        // in cas-code-review. Pin content-identity at the source, modulo
        // the intentional per-harness tool prefix (cas-2c61: the codex
        // copy correctly uses mcp__cs__, not Claude's mcp__cas__).
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/session-learn/SKILL.md")
            .expect("BUILTIN_SKILLS missing session-learn SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/session-learn/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing session-learn SKILL.md");
        assert_eq!(
            claude.content.replace("mcp__cas__", "mcp__cs__"),
            codex.content,
            "session-learn SKILL.md .claude and .codex copies must be identical apart from \
             the mcp__cas__/mcp__cs__ tool prefix; drift here produces a divergent \
             classifier prompt across harnesses",
        );
    }

    #[test]
    fn test_cas_code_review_skill_md_mirrors_are_identical() {
        // The .claude and .codex builtin copies of cas-code-review/SKILL.md
        // are sync-mirrored by `cas update`. Drift between them
        // re-introduces the cas-ec8f regression on whichever harness reads
        // the stale copy — guard against that at the source, modulo the
        // intentional per-harness tool prefix (cas-2c61: the codex copy
        // correctly uses mcp__cs__, not Claude's mcp__cas__).
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-code-review/SKILL.md")
            .expect("BUILTIN_SKILLS missing cas-code-review SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-code-review/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing cas-code-review SKILL.md");
        assert_eq!(
            claude.content.replace("mcp__cas__", "mcp__cs__"),
            codex.content,
            "cas-code-review SKILL.md .claude and .codex copies must be identical apart \
             from the mcp__cas__/mcp__cs__ tool prefix; drift here re-opens cas-ec8f on \
             the harness reading the stale copy",
        );
    }

    #[test]
    fn test_cas_code_review_documents_codex_transport_and_independent_persona() {
        for (label, skills) in [("claude", BUILTIN_SKILLS), ("codex", CODEX_BUILTIN_SKILLS)] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-code-review/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: skills/cas-code-review/SKILL.md missing"));
            for required in [
                "gpt-5.6-sol:independent",
                "one shared schema shim",
                "codex exec -s read-only -m gpt-5.6-sol -c model_reasoning_effort=high",
                "`--output-schema`",
                "two bounded schema-mismatch retries",
                "one separately-budgeted timeout retry",
                "600 seconds",
                "Security deliberately stays on Claude Opus",
                "5+ changed files",
                "300+ changed lines",
                "gpt56_independent: true",
                "legacy `gpt55_independent: true` remains accepted",
                "skipped_reason",
                "distinct from a successful zero-finding review",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label}: cas-code-review SKILL.md missing Codex transport/independent persona marker: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_cas_code_review_documents_dropped_findings_contract() {
        for (label, skills) in [
            ("claude", BUILTIN_SKILLS),
            ("codex", CODEX_BUILTIN_SKILLS),
            ("grok", GROK_BUILTIN_SKILLS),
        ] {
            let entry = skills
                .iter()
                .find(|b| b.path == "skills/cas-code-review/SKILL.md")
                .unwrap_or_else(|| panic!("{label}: skills/cas-code-review/SKILL.md missing"));
            for required in [
                "\"dropped\": DroppedFinding[]",
                "schema_validation_failed",
                "validation_errors",
                "confidence_below_threshold",
                "stats.dropped_findings",
                "exactly `dropped.length`",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label}: cas-code-review SKILL.md missing dropped-finding contract marker: {required:?}"
                );
            }
        }
    }

    #[test]
    fn test_code_reviewer_agent_is_deprecation_stub() {
        // EPIC cas-0750: the legacy code-reviewer agent is replaced by the
        // cas-code-review skill. The file is kept in BUILTIN_AGENTS only to
        // propagate a deprecation stub via `cas sync`.
        for agents in [BUILTIN_AGENTS, CODEX_BUILTIN_AGENTS] {
            let entry = agents
                .iter()
                .find(|b| b.path == "agents/code-reviewer.md")
                .expect("code-reviewer.md must remain in the builtins list so sync overwrites downstream copies");
            assert!(
                entry.content.contains("deprecated: true"),
                "code-reviewer.md must carry `deprecated: true` in frontmatter"
            );
            assert!(
                entry.content.contains("replaced_by: cas-code-review"),
                "code-reviewer.md must name its replacement"
            );
            assert!(
                entry.content.contains("managed_by: cas"),
                "code-reviewer.md must keep `managed_by: cas` so sync overwrites stale copies"
            );
            assert!(
                entry.content.contains("DEPRECATED"),
                "code-reviewer.md must prominently mark itself as deprecated"
            );
        }
    }

    /// cas-0e5b3 (GH #112): the checked-in rendered copy must equal the builtin
    /// it is generated from.
    ///
    /// `.claude/workflows/cas-code-review.js` is not a second source — it is
    /// output that [`sync_workflows`] force-writes from [`BUILTIN_WORKFLOWS`].
    /// A one-line divergence (`CODEX_PERSONA_EFFORT`) sat in the tree for a
    /// week because the only guard for it lived in the Node test suite, which
    /// `cargo test` never runs: every worker and every CI pass was green while
    /// this repo reviewed its own code at a different reasoning effort than the
    /// one it ships. This test is deliberately standalone and named for what it
    /// checks, rather than riding along inside a sync test, because the whole
    /// failure mode was drift going unnoticed.
    #[test]
    fn test_rendered_workflow_matches_builtin() {
        // Coupled to the single BUILTIN_WORKFLOWS entry: `include_str!` needs a
        // literal path, so this cannot iterate the slice. The length assert
        // below turns "someone added a second workflow" into a loud failure
        // here instead of a silently unguarded file.
        assert_eq!(
            BUILTIN_WORKFLOWS.len(),
            1,
            "a new builtin workflow was added but this parity guard still checks only \
             cas-code-review.js — extend it with an include_str! for the new file, or \
             the new rendered copy can drift undetected (cas-0e5b3)"
        );

        let rendered_copy = include_str!("../../.claude/workflows/cas-code-review.js");
        let builtin_source = BUILTIN_WORKFLOWS
            .iter()
            .find(|f| f.path == "workflows/cas-code-review.js")
            .expect("cas-code-review workflow must be a builtin")
            .content;
        if rendered_copy != builtin_source {
            // Name the first divergent line: the previous guard dumped both
            // whole files, which is how a 1-in-1100-line difference stayed
            // invisible even after the test went red.
            let first_diff = rendered_copy
                .lines()
                .zip(builtin_source.lines())
                .position(|(a, b)| a != b);
            let detail = match first_diff {
                Some(i) => format!(
                    "line {}:\n  builtin (source of truth): {}\n  rendered copy (stale):     {}",
                    i + 1,
                    builtin_source.lines().nth(i).unwrap_or("<absent>"),
                    rendered_copy.lines().nth(i).unwrap_or("<absent>"),
                ),
                None => "one file is a prefix of the other (trailing content differs)".to_string(),
            };
            panic!(
                ".claude/workflows/cas-code-review.js is STALE relative to its builtin \
                 source.\nThat file is generated — do NOT edit it to fix this.\n{detail}\n\
                 Repair: edit cas-cli/src/builtins/workflows/cas-code-review.js, then \
                 regenerate with `cas update --sync` (or copy builtin -> .claude/workflows/)."
            );
        }
    }

    #[test]
    fn test_sync_installs_cas_code_review_and_overwrites_code_reviewer() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Pre-seed a stale copy of the old agent to prove sync overwrites it.
        let stale_agent = claude_dir.join("agents/code-reviewer.md");
        std::fs::create_dir_all(stale_agent.parent().unwrap()).unwrap();
        std::fs::write(
            &stale_agent,
            "---\nname: code-reviewer\nmanaged_by: cas\n---\nold content",
        )
        .unwrap();

        sync_all_builtins(&claude_dir).unwrap();

        for p in [
            "skills/cas-code-review/SKILL.md",
            "skills/cas-code-review/references/findings-schema.md",
            "skills/cas-code-review/references/personas/correctness.md",
            "skills/cas-code-review/references/personas/testing.md",
            "skills/cas-code-review/references/personas/maintainability.md",
            "skills/cas-code-review/references/personas/project-standards.md",
            "skills/cas-code-review/references/personas/security.md",
            "skills/cas-code-review/references/personas/performance.md",
            "skills/cas-code-review/references/personas/adversarial.md",
            // Phase B (cas-b667): production Workflow shipped via BUILTIN_WORKFLOWS
            "workflows/cas-code-review.js",
        ] {
            let f = claude_dir.join(p);
            assert!(f.exists(), "{p} not synced");
        }

        // Phase B: verify the workflow content is the production script
        let workflow_content =
            std::fs::read_to_string(claude_dir.join("workflows/cas-code-review.js"))
                .expect("workflow script must be synced");
        assert!(
            workflow_content.contains("cas-code-review"),
            "workflow script must reference cas-code-review"
        );
        assert!(
            workflow_content.contains("mergeFindings"),
            "workflow script must contain the mergeFindings() pipeline"
        );
        assert!(
            workflow_content.contains("REVIEWER_OUTPUT_SCHEMA"),
            "workflow script must define the reviewer output schema"
        );
        for required in [
            "gpt-5.6-sol:independent",
            "gpt56_independent",
            "gpt55_independent",
            "gpt55ShouldRun",
            "function buildCodexReviewerShimPrompt",
            "async function dispatchReviewPersona",
            "codex exec -s read-only -m ${CODEX_PERSONA_MODEL} -c model_reasoning_effort=${CODEX_PERSONA_EFFORT}",
            "--output-schema",
            "CODEX_PERSONA_TIMEOUT_SECONDS = 600",
            "CODEX_SCHEMA_RETRIES = 2",
            "CODEX_TIMEOUT_RETRIES = 1",
            "CODEX_MAX_CONCURRENCY = 4",
            "Schema mismatch retries do not consume the timeout retry budget",
            "required: ['reviewer', 'findings', 'residual_risks', 'testing_gaps', 'skipped_reason']",
            "required: ['title','severity','file','line','why_it_matters','autofix_class','owner','confidence','evidence','pre_existing','suggested_fix','requires_verification']",
            "function stripNullValues",
            "function skippedPersonaResults",
            "function incompleteAlwaysOnPersonas",
            "async function pipelineWithConcurrency",
            "command -v sleep",
            "TIMEOUT_MARKER",
            "CLAUDE_DIVERSITY_PERSONA = 'security'",
            "model: 'opus'",
            "skipped_reason",
            "gpt55_independent_skipped",
            "skipped_personas",
            "status: reviewStatus",
            "degraded: incompletePersonas.length > 0",
        ] {
            assert!(
                workflow_content.contains(required),
                "workflow script missing shared Codex transport marker: {required:?}"
            );
        }
        assert_eq!(
            workflow_content
                .matches("codex exec -s read-only -m ${CODEX_PERSONA_MODEL} -c model_reasoning_effort=${CODEX_PERSONA_EFFORT}")
                .count(),
            1,
            "the Codex command must live once in the shared reviewer shim"
        );
        assert!(
            !workflow_content.contains("/usr/bin/timeout"),
            "cas-code-review must use the portable shell watchdog, not GNU timeout"
        );
        assert!(
            !workflow_content.contains("model: 'sonnet'"),
            "retired Sonnet model dispatch must not remain in cas-code-review"
        );
        let constants_content =
            include_str!("../../.claude/workflows/cas-code-review-constants.js");
        for helper in [
            "gpt55ShouldRun",
            "stripNullValues",
            "skippedPersonaResults",
            "personasRunCount",
            "incompleteAlwaysOnPersonas",
        ] {
            assert_eq!(
                extract_js_function(&workflow_content, helper),
                extract_js_function(constants_content, helper),
                "workflow inline helper {helper} must match cas-code-review-constants.js"
            );
        }

        let overwritten = std::fs::read_to_string(&stale_agent).unwrap();
        assert!(
            overwritten.contains("DEPRECATED"),
            "sync must overwrite the stale code-reviewer.md with the deprecation stub"
        );
        assert!(
            overwritten.contains("replaced_by: cas-code-review"),
            "deprecation stub must name the replacement"
        );
    }

    #[test]
    fn test_builtin_agents_contains_task_verifier() {
        // Verify task-verifier agent is in BUILTIN_AGENTS and will be synced
        let has_task_verifier = BUILTIN_AGENTS
            .iter()
            .any(|b| b.path == "agents/task-verifier.md");
        assert!(
            has_task_verifier,
            "task-verifier.md must be in BUILTIN_AGENTS for cas init to sync it"
        );
    }

    #[test]
    fn test_task_verifier_has_correct_frontmatter() {
        // Verify task-verifier content has required frontmatter fields
        let task_verifier = BUILTIN_AGENTS
            .iter()
            .find(|b| b.path.contains("task-verifier"))
            .expect("task-verifier must exist in BUILTIN_AGENTS");

        assert!(
            task_verifier.content.contains("name: task-verifier"),
            "task-verifier must have name in frontmatter"
        );
        assert!(
            task_verifier.content.contains("managed_by: cas"),
            "task-verifier must be marked as managed by Cassy"
        );
        assert!(
            task_verifier.content.contains("description:"),
            "task-verifier must have description"
        );
    }

    /// cas-4900 regression: `sync_all_builtins` was reported to silently
    /// skip reference files (anything under `<skill>/references/*.md`)
    /// when invoked against a project-style target_dir, even though the
    /// same code path worked against `~/.claude` (user-level). This test
    /// runs the same `sync_all_builtins` function against a tempdir that
    /// has been pre-populated with stale content for a reference file,
    /// asserts the stale content gets overwritten with fresh source, and
    /// asserts a separately-deleted reference file gets recreated.
    ///
    /// If this test PASSES on main, `sync_all_builtins` itself is innocent
    /// and the bug must live in the orchestration around it
    /// (`sync_claude_files` in `cli/update.rs`), most likely the
    /// `SkillSyncer::sync_all` invocation that runs immediately before.
    /// The locked-in assertion here is the safety net: any future
    /// refactor that breaks reference-file write logic at this layer
    /// fails this test loudly instead of slipping into silent staleness.
    #[test]
    fn test_sync_all_builtins_overwrites_stale_and_recreates_deleted_reference_files() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        // Initial sync — populate everything fresh.
        sync_all_builtins(&claude_dir).unwrap();

        // Pick two real reference files that exist in BUILTIN_SKILLS today.
        // Both carry `managed_by: cas` frontmatter (planning.md was the
        // exemplar in the 2026-05-06 cas-4900 repro).
        let planning_path = claude_dir.join("skills/cas-supervisor/references/planning.md");
        let close_gate_path = claude_dir.join("skills/cas-worker/references/close-gate.md");
        assert!(
            planning_path.exists(),
            "initial sync must have written planning.md"
        );
        assert!(
            close_gate_path.exists(),
            "initial sync must have written close-gate.md"
        );

        let planning_src = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/planning.md")
            .expect("planning.md must be registered in BUILTIN_SKILLS")
            .content;

        // Stage 1: model a reference installed by an older Cassy version:
        // destination content and the last-synced ledger both carry the old
        // source hash. This is deliberately distinct from a local edit, where
        // the destination would diverge from the ledger and must be preserved.
        let stale_marker = "STALE Cassy-4900 SENTINEL — should be overwritten on next sync";
        let stale_content =
            format!("---\nname: planning\nmanaged_by: cas\n---\n\n{stale_marker}\n");
        std::fs::write(&planning_path, &stale_content).unwrap();
        let mut reference_state = BuiltinReferenceState::load(&claude_dir).unwrap();
        reference_state.files.insert(
            "skills/cas-supervisor/references/planning.md".to_string(),
            builtin_content_hash(&stale_content),
        );
        reference_state.save(&claude_dir).unwrap();

        // Stage 2: delete close-gate.md outright and capture that intent
        // before modeling database skill sync rehydrating an old copy.
        std::fs::remove_file(&close_gate_path).unwrap();
        assert!(
            !close_gate_path.exists(),
            "precondition: deletion took effect"
        );
        assert_eq!(
            mark_missing_owned_references_for_replacement(
                SupervisorCli::Claude,
                &claude_dir
            )
            .unwrap(),
            1,
            "the deleted real reference must be marked for one-shot replacement"
        );
        std::fs::write(&close_gate_path, "# stale database-skill copy\n").unwrap();

        // Re-run sync. The one-shot marker must win over the intervening
        // database copy, matching the real `cas update --sync` ordering.
        let result = sync_all_builtins(&claude_dir).unwrap();

        // Recreation invariant.
        assert!(
            close_gate_path.exists(),
            "cas-4900 regression: sync_all_builtins did NOT recreate the \
             deleted close-gate.md reference file"
        );
        let close_gate_after = std::fs::read_to_string(&close_gate_path).unwrap();
        assert!(
            close_gate_after.contains("managed_by: cas"),
            "recreated close-gate.md must carry the source frontmatter"
        );

        // Overwrite invariant.
        let planning_after = std::fs::read_to_string(&planning_path).unwrap();
        assert!(
            !planning_after.contains(stale_marker),
            "cas-4900 regression: sync_all_builtins did NOT overwrite the \
             stale planning.md reference file"
        );
        assert_eq!(
            planning_after, planning_src,
            "planning.md must match the BUILTIN_SKILLS source byte-for-byte after sync"
        );

        // Update count must reflect both files (recreated + overwritten).
        // Other built-ins were already current after the initial sync, so
        // the second-sync update count should be exactly 2.
        assert_eq!(
            result.total_updated(),
            2,
            "second sync should report exactly 2 updated files (the \
             recreated close-gate.md + the overwritten planning.md); got: {:?}",
            result.updated_files,
        );
    }

    /// cas-4900 surfacing: when the destination has an *unmanaged* file
    /// whose content differs from the source AND the source is also
    /// unmanaged, the gate correctly refuses to overwrite — but the
    /// outcome must be observable. Pin the `SkippedNotManaged` variant
    /// and the population of `SyncResult::skipped_files` so future
    /// refactors can't slip back into the pre-9362ee0 silent-skip mode.
    ///
    /// Note: with current `BUILTIN_SKILLS` content (post-9362ee0 — every
    /// builtin file carries `managed_by: cas`), this gate is effectively
    /// untriggerable in production via the real builtins. The test
    /// constructs a synthetic `BuiltinFile` whose source content lacks
    /// the marker so we can exercise the path. This is the regression
    /// safety net for the OTHER half of cas-4900 (the AC bullet
    /// "Reference files WITHOUT the marker either sync correctly OR
    /// emit a clear warning so silent-skip is no longer possible").
    #[test]
    fn test_sync_builtin_detailed_surfaces_silent_skip_for_unmanaged_drift() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let target_dir = temp.path();

        // Synthetic builtin whose source has NO managed_by marker — the
        // exact case the pre-9362ee0 gate would silently swallow.
        let synthetic = BuiltinFile {
            path: "skills/cas-test-synthetic/references/example.md",
            content: "# Synthetic ref file — unmanaged source\n\nupdated body\n",
        };

        // Seed destination with DIFFERENT unmanaged content. The gate
        // must refuse to overwrite (preserves user content) AND must
        // signal SkippedNotManaged so the caller can warn.
        let target_path = target_dir.join(synthetic.path);
        std::fs::create_dir_all(target_path.parent().unwrap()).unwrap();
        std::fs::write(&target_path, "# Different unmanaged content\n").unwrap();

        let outcome = sync_builtin_detailed(&synthetic, target_dir).unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::SkippedNotManaged,
            "drift between unmanaged source and unmanaged dest must surface as \
             SkippedNotManaged, not collapse into a silent false return"
        );
        assert!(
            !outcome.wrote(),
            "SkippedNotManaged must be a no-write outcome (preserves user content)"
        );

        // Identical unmanaged content → Unchanged (genuine no-op,
        // distinct from SkippedNotManaged so callers don't false-positive
        // warn on the happy path).
        std::fs::write(&target_path, synthetic.content).unwrap();
        let outcome = sync_builtin_detailed(&synthetic, target_dir).unwrap();
        assert_eq!(
            outcome,
            SyncOutcome::Unchanged,
            "matching unmanaged content must surface as Unchanged, not \
             SkippedNotManaged — surfacing it would noise up the warn channel"
        );
    }

    /// cas-4900 regression: `SyncResult::skipped_files` must be populated
    /// when the inner sync loop encounters a `SkippedNotManaged` outcome,
    /// and `has_silent_skips()` must report it. This is what the
    /// `cas update --sync` CLI surfacing reads from to print warnings.
    #[test]
    fn test_sync_result_tracks_silent_skips_for_cli_surfacing() {
        let mut result = SyncResult::default();
        assert!(!result.has_silent_skips());
        result
            .skipped_files
            .push("skills/foo/references/bar.md".to_string());
        assert!(
            result.has_silent_skips(),
            "any populated skipped_files entry must flip has_silent_skips() to true"
        );
        assert_eq!(result.skipped_files.len(), 1);
    }

    #[test]
    fn managed_skill_owns_unmarked_references_without_clobbering_local_changes() {
        use tempfile::tempdir;

        const BODY: &str = "---\nname: cas-test\nmanaged_by: cas\n---\n# Test\n";
        const REFERENCE_V1: &str = "# Reference\n\nversion one\n";
        const REFERENCE_V2: &str = "# Reference\n\nversion two\n";
        const REFERENCE_V3: &str = "# Reference\n\nversion three\n";
        const LOCAL_EDIT: &str = "# Reference\n\nproject customization\n";
        const BODY_FILE: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/SKILL.md",
            content: BODY,
        };
        const REFERENCE_FILE_V1: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/references/new-reference.md",
            content: REFERENCE_V1,
        };
        const REFERENCE_FILE_V2: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/references/new-reference.md",
            content: REFERENCE_V2,
        };
        const REFERENCE_FILE_V3: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/references/new-reference.md",
            content: REFERENCE_V3,
        };

        let temp = tempdir().unwrap();
        let target_dir = temp.path().join(".claude");
        let target_reference = target_dir.join(REFERENCE_FILE_V1.path);

        let first =
            sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, REFERENCE_FILE_V1]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target_reference).unwrap(),
            REFERENCE_V1,
            "a new reference without frontmatter must install with its managed skill"
        );
        assert!(
            first
                .updated_files
                .contains(&REFERENCE_FILE_V1.path.to_string()),
            "fresh sync must report the body-owned reference as created"
        );

        let second =
            sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, REFERENCE_FILE_V2]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target_reference).unwrap(),
            REFERENCE_V2,
            "an unchanged downstream reference must receive later source updates"
        );
        assert!(
            second
                .updated_files
                .contains(&REFERENCE_FILE_V2.path.to_string()),
            "propagated reference update must be visible in the sync result"
        );

        std::fs::write(&target_reference, LOCAL_EDIT).unwrap();
        let third =
            sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, REFERENCE_FILE_V3]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target_reference).unwrap(),
            LOCAL_EDIT,
            "a locally customized reference must never be silently overwritten"
        );
        assert_eq!(
            third.modified_reference_files,
            vec![REFERENCE_FILE_V3.path.to_string()],
            "the preserved customization must be surfaced to the caller"
        );

        std::fs::remove_file(&target_reference).unwrap();
        let remediated =
            sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, REFERENCE_FILE_V3]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target_reference).unwrap(),
            REFERENCE_V3,
            "delete-and-resync remediation must install and baseline the current Cassy reference"
        );
        assert!(
            remediated.modified_reference_files.is_empty(),
            "a remediated reference must no longer report a customization conflict"
        );
    }

    /// cas-0c0a: a destination with no recorded baseline whose content is a
    /// version Cassy previously shipped is stale Cassy content, not a local edit.
    /// Before this, every pre-ledger install kept its old copy forever while
    /// `cas update --sync` reported success.
    #[test]
    fn pre_ledger_reference_matching_a_shipped_version_is_upgraded_and_baselined() {
        use tempfile::tempdir;

        const BODY: &str = "---\nname: cas-test\nmanaged_by: cas\n---\n# Test\n";
        const SHIPPED_OLD: &str = "# Reference\n\nshipped june\n";
        const SHIPPED_NEW: &str = "# Reference\n\nshipped august\n";
        const PATH: &str = "skills/cas-test/references/pre-ledger.md";
        const BODY_FILE: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/SKILL.md",
            content: BODY,
        };
        const NEW_FILE: BuiltinFile = BuiltinFile {
            path: PATH,
            content: SHIPPED_NEW,
        };

        let temp = tempdir().unwrap();
        let target_dir = temp.path().join(".claude");
        let target = target_dir.join(PATH);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        // Pre-ledger world: old shipped content on disk, no baseline recorded.
        std::fs::write(&target, SHIPPED_OLD).unwrap();

        set_history_override(PATH, &[SHIPPED_OLD]);
        let result = sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, NEW_FILE]).unwrap();
        clear_history_override();

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            SHIPPED_NEW,
            "a pre-ledger destination matching an older shipped version must be upgraded"
        );
        assert!(
            result.modified_reference_files.is_empty(),
            "an upgraded stale reference is not a customization conflict"
        );
        let state = BuiltinReferenceState::load(&target_dir).unwrap();
        assert_eq!(
            state.files.get(PATH),
            Some(&builtin_content_hash(SHIPPED_NEW)),
            "the upgrade must baseline the new content so later syncs propagate cleanly"
        );
        assert!(
            state.skipped_references.is_empty(),
            "an upgraded reference must not be recorded as skipped"
        );
    }

    /// cas-0c0a companion: content matching *no* shipped version is a genuine
    /// local customization — preserved, and recorded so SessionStart can say so.
    #[test]
    fn unknown_reference_content_is_preserved_and_recorded_for_the_session_banner() {
        use tempfile::tempdir;

        const BODY: &str = "---\nname: cas-test\nmanaged_by: cas\n---\n# Test\n";
        const SHIPPED_OLD: &str = "# Reference\n\nshipped june\n";
        const SHIPPED_NEW: &str = "# Reference\n\nshipped august\n";
        const LOCAL_EDIT: &str = "# Reference\n\nour own house rules\n";
        const PATH: &str = "skills/cas-test/references/customized.md";
        const BODY_FILE: BuiltinFile = BuiltinFile {
            path: "skills/cas-test/SKILL.md",
            content: BODY,
        };
        const NEW_FILE: BuiltinFile = BuiltinFile {
            path: PATH,
            content: SHIPPED_NEW,
        };

        let temp = tempdir().unwrap();
        let target_dir = temp.path().join(".claude");
        let target = target_dir.join(PATH);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        // The per-harness ledger only lands under <repo>/.cas/cache when a
        // `.cas` directory exists — that is the layout SessionStart reads.
        std::fs::create_dir_all(temp.path().join(".cas")).unwrap();
        std::fs::write(&target, LOCAL_EDIT).unwrap();

        set_history_override(PATH, &[SHIPPED_OLD]);
        let result = sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, NEW_FILE]).unwrap();
        clear_history_override();

        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            LOCAL_EDIT,
            "content matching no shipped Cassy version must still be preserved"
        );
        assert_eq!(
            result.modified_reference_files,
            vec![PATH.to_string()],
            "the preserved customization must be reported to the caller"
        );

        let state = BuiltinReferenceState::load(&target_dir).unwrap();
        assert!(
            state.skipped_references.contains_key(PATH),
            "the skip must be persisted so SessionStart can surface it"
        );

        // The ledger for `.claude` lives under <cas_root>/cache/... and is what
        // the SessionStart banner reads.
        let cas_root = temp.path().join(".cas");
        let skipped = skipped_owned_references(&cas_root);
        assert_eq!(
            skipped.get("claude").map(Vec::as_slice),
            Some([PATH.to_string()].as_slice()),
            "skipped references must be readable per harness from the cas cache"
        );
        let banner =
            crate::hooks::handlers::session_hygiene::render_stale_reference_banner(&skipped);
        assert!(
            banner.full.contains(PATH) && banner.full.contains("cas update --sync"),
            "the banner must name the skipped path and the acceptance remediation: {}",
            banner.full
        );
        assert!(
            banner.compact.contains("cas update --sync"),
            "the degraded rendering must still carry the remediation command"
        );

        // Once the local edit is reconciled, the banner must fall silent.
        std::fs::write(&target, SHIPPED_NEW).unwrap();
        sync_all_builtins_inner(&target_dir, &[], &[BODY_FILE, NEW_FILE]).unwrap();
        assert!(
            skipped_owned_references(&cas_root).is_empty(),
            "a resolved reference must clear the persisted skip"
        );
    }

    /// The embedded history is the whole mechanism — an empty or unparseable
    /// ledger silently reverts to the pre-fix behaviour, so assert it is real.
    #[test]
    fn embedded_builtin_reference_history_covers_shipped_reference_paths() {
        let history = builtin_reference_history();
        assert!(
            history.files.len() >= 20,
            "embedded reference history looks truncated ({} paths); rerun \
             scripts/gen-builtin-reference-history.sh",
            history.files.len()
        );
        for path in [
            "skills/cas-supervisor/references/worker-recovery.md",
            "skills/cas-worker/references/recovery.md",
        ] {
            let hashes = history
                .files
                .get(path)
                .unwrap_or_else(|| panic!("no shipped-version history recorded for {path}"));
            assert!(
                hashes.len() >= 2,
                "{path} should have multiple shipped versions recorded, got {}",
                hashes.len()
            );
            assert!(
                hashes.iter().all(|h| h.len() == 64
                    && h.chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
                "{path} history must contain lowercase sha256 hex digests"
            );
        }
    }

    #[test]
    fn test_sync_all_builtins_includes_compound_engineering() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        sync_all_builtins(&claude_dir).unwrap();
        for p in [
            "agents/git-history-analyzer.md",
            "agents/issue-intelligence-analyst.md",
            "skills/cas-brainstorm/SKILL.md",
            "skills/cas-brainstorm/references/handoff.md",
            "skills/cas-brainstorm/references/requirements-capture.md",
            "skills/cas-ideate/SKILL.md",
            "skills/cas-ideate/references/post-ideation-workflow.md",
        ] {
            let f = claude_dir.join(p);
            assert!(f.exists(), "{} not synced", p);
            let body = std::fs::read_to_string(&f).unwrap();
            assert!(
                body.contains("managed_by: cas"),
                "{} missing managed_by: cas",
                p
            );
        }
    }

    #[test]
    fn test_sync_all_builtins_includes_agents() {
        // Verify sync_all_builtins syncs agents (which includes task-verifier)
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let claude_dir = temp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();

        let result = sync_all_builtins(&claude_dir).unwrap();

        // Should sync at least 1 agent (task-verifier)
        assert!(
            result.agents_updated > 0,
            "sync_all_builtins should sync agents"
        );

        // Verify task-verifier file was created
        let task_verifier_path = claude_dir.join("agents/task-verifier.md");
        assert!(
            task_verifier_path.exists(),
            "task-verifier.md should be created by sync_all_builtins"
        );
    }

    #[test]
    fn test_builtin_skills_contains_cas_nuxt_playwright() {
        let expected = [
            "skills/cas-nuxt-playwright/SKILL.md",
            "skills/cas-nuxt-playwright/references/auth-fixture-template.md",
        ];
        for p in expected {
            assert!(
                BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from BUILTIN_SKILLS"
            );
            assert!(
                CODEX_BUILTIN_SKILLS.iter().any(|b| b.path == p),
                "{p} missing from CODEX_BUILTIN_SKILLS"
            );
        }

        let entry = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-nuxt-playwright/SKILL.md")
            .unwrap();
        for required in [
            "name: cas-nuxt-playwright",
            "managed_by: cas",
            "navigateTo",
            "window.__nuxt",
            "IndexedDB",
            "ssr: false",
            "routeRules",
            "q-btn",
        ] {
            assert!(
                entry.content.contains(required),
                "cas-nuxt-playwright SKILL.md missing required marker: {required:?}"
            );
        }
    }

    #[test]
    fn test_cas_nuxt_playwright_mirrors_are_identical() {
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-nuxt-playwright/SKILL.md")
            .expect("BUILTIN_SKILLS missing cas-nuxt-playwright SKILL.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-nuxt-playwright/SKILL.md")
            .expect("CODEX_BUILTIN_SKILLS missing cas-nuxt-playwright SKILL.md");
        assert_eq!(
            claude.content, codex.content,
            "cas-nuxt-playwright SKILL.md .claude and .codex copies must be byte-identical",
        );

        let claude_ref = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-nuxt-playwright/references/auth-fixture-template.md")
            .expect("BUILTIN_SKILLS missing auth-fixture-template.md");
        let codex_ref = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-nuxt-playwright/references/auth-fixture-template.md")
            .expect("CODEX_BUILTIN_SKILLS missing auth-fixture-template.md");
        assert_eq!(
            claude_ref.content, codex_ref.content,
            "auth-fixture-template.md .claude and .codex copies must be byte-identical",
        );
    }

    /// cas-6219: the supervisor's model-selection rubric must be registered on
    /// both surfaces, stay content-identical across mirrors modulo the
    /// intentional per-harness tool prefix (cas-2c61/cas-62ab: the codex copy
    /// correctly uses mcp__cs__, not Claude's mcp__cas__), and remain
    /// discoverable from the skill body that fits the 8 KB cap.
    #[test]
    fn test_supervisor_model_selection_reference_registered_and_mirrored() {
        let claude = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/model-selection.md")
            .expect("BUILTIN_SKILLS missing cas-supervisor model-selection.md");
        let codex = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/model-selection.md")
            .expect("CODEX_BUILTIN_SKILLS missing cas-supervisor model-selection.md");
        let grok = GROK_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/model-selection.md")
            .expect("GROK_BUILTIN_SKILLS missing cas-supervisor model-selection.md");
        assert_eq!(
            claude.content.replace("mcp__cas__", "mcp__cs__"),
            codex.content,
            "model-selection.md .claude and .codex copies must be identical apart from \
             the mcp__cas__/mcp__cs__ tool prefix",
        );
        // cas-b342: the Grok twin is a third normalized mirror — identical to
        // the Claude copy apart from the cas__ tool prefix.
        assert_eq!(
            claude.content.replace("mcp__cas__", "cas__"),
            grok.content,
            "model-selection.md .claude and .grok copies must be identical apart from \
             the mcp__cas__/cas__ tool prefix",
        );
        // The Codex-first tier table is the contract: Grok Composer serves
        // genuinely light work, Luna/xhigh is standard and taste, while
        // Sol/high is heavy/frontier only and Terra is suspended.
        for required in [
            // Codex-first tier table (AC2)
            "Codex-first",
            "cli=grok model=grok-composer-2.5-fast effort=low",
            "cli=codex model=gpt-5.6-luna effort=xhigh",
            "cli=codex model=gpt-5.6-sol effort=high",
            "gpt-5.6-luna",
            "gpt-5.4-mini successor",
            "Sonnet is not a normal worker lane",
            "not Sonnet",
            // Claude Opus exceptional lane (AC3 revised)
            "Claude Opus",
            "cli=claude model=opus effort=high",
            "architecture",
            "safety",
            "rescue",
            "independent challenge",
            // Grok capacity lane (AC4). The light Grok recipe is pinned with
            // explicit effort=low — every spawn recipe must carry cli/model/effort.
            "capacity",
            "cli=grok model=grok-composer-2.5-fast effort=low",
            "grok model=grok-4.5",
            "same-tier Codex fallback",
            "health",
            // Rubric contract carried over
            "light",
            "standard",
            "heavy",
            "frontier",
            "tier:",
            "Escalate on failure",
            "Routing Axes",
            "Cost",
            "Intelligence",
            "Taste",
            "Taste-sensitive work routes to Codex GPT-5.6 Luna at",
            "effort=high",
            "Terra is suspended",
            "operator decision pending",
            "Escalate on judgment",
            "Cost is a tiebreaker only",
        ] {
            assert!(
                claude.content.contains(required),
                "model-selection.md missing required tier-rubric marker: {required:?}"
            );
        }
        // cas-b342 edge case: the exact frontier slug is `gpt-5.6-sol`; a bare
        // `gpt-5.6` must never appear as a spawn recipe (`model=gpt-5.6` or the
        // `codex/gpt-5.6` tier shorthand). Documentation may still mention the
        // bare slug to warn against it, so we only forbid the recipe forms.
        let stripped = claude
            .content
            .replace("gpt-5.6-sol", "SOLSLUG")
            .replace("gpt-5.6-terra", "TERRASLUG")
            .replace("gpt-5.6-luna", "LUNASLUG");
        assert!(
            !stripped.contains("model=gpt-5.6") && !stripped.contains("codex/gpt-5.6"),
            "model-selection.md must not use a bare gpt-5.6 spawn recipe"
        );
        // cas-b4ea: GPT-5.5 is no longer a positive supervisor worker route.
        // Keep this scoped to the supervisor rubric files so unrelated
        // code-review/cas-codex-exec persona tests may continue documenting
        // their own model choices.
        for (label, content) in [
            ("claude body", SUPERVISOR_GUIDE),
            (
                "claude model-selection",
                include_str!("builtins/skills/cas-supervisor/references/model-selection.md"),
            ),
            (
                "claude workflow",
                include_str!("builtins/skills/cas-supervisor/references/workflow.md"),
            ),
            (
                "claude reference",
                include_str!("builtins/skills/cas-supervisor/references/reference.md"),
            ),
            (
                "codex body",
                include_str!("builtins/codex/skills/cas-supervisor.md"),
            ),
            (
                "codex model-selection",
                include_str!("builtins/codex/skills/cas-supervisor/references/model-selection.md"),
            ),
            (
                "codex workflow",
                include_str!("builtins/codex/skills/cas-supervisor/references/workflow.md"),
            ),
            (
                "codex reference",
                include_str!("builtins/codex/skills/cas-supervisor/references/reference.md"),
            ),
            (
                "grok body",
                include_str!("builtins/grok/skills/cas-supervisor.md"),
            ),
            (
                "grok model-selection",
                include_str!("builtins/grok/skills/cas-supervisor/references/model-selection.md"),
            ),
            (
                "grok workflow",
                include_str!("builtins/grok/skills/cas-supervisor/references/workflow.md"),
            ),
            (
                "grok reference",
                include_str!("builtins/grok/skills/cas-supervisor/references/reference.md"),
            ),
        ] {
            assert!(
                !content.contains("model=gpt-5.5") && !content.contains("codex/gpt-5.5"),
                "{label} must not contain a GPT-5.5 supervisor worker recipe"
            );
        }
        // cas-b342/cas-96ea: the hard rule requires explicit cli/model/effort on EVERY
        // spawn, so every `spawn_workers` recipe line in the rubric — including
        // the light Grok Composer lane — must carry an explicit `effort=`, and
        // Sonnet must not remain as a copyable spawn recipe.
        for line in claude.content.lines() {
            if line.contains("action=spawn_workers") {
                assert!(
                    line.contains("effort="),
                    "spawn_workers recipe omits explicit effort=: {line:?}"
                );
                assert!(
                    !line.contains("model=sonnet"),
                    "supervisor model-selection.md must not contain a Sonnet spawn recipe: {line:?}"
                );
            }
        }
        // Discoverable from the SessionStart-injected body on all three surfaces.
        for (label, guide) in [
            ("claude cas-supervisor.md", SUPERVISOR_GUIDE),
            (
                "codex cas-supervisor.md",
                include_str!("builtins/codex/skills/cas-supervisor.md"),
            ),
            (
                "grok cas-supervisor.md",
                include_str!("builtins/grok/skills/cas-supervisor.md"),
            ),
        ] {
            assert!(
                guide.contains("references/model-selection.md"),
                "{label} must point at the model-selection rubric"
            );
        }
    }

    /// cas-7199c: copyable supervisor commands and reference twins must stay
    /// complete across every harness surface, not only model-selection.md.
    #[test]
    fn test_supervisor_rubric_recipes_and_reference_twins_stay_normalized() {
        let claude_body = SUPERVISOR_GUIDE;
        let claude_model =
            include_str!("builtins/skills/cas-supervisor/references/model-selection.md");
        let claude_workflow = include_str!("builtins/skills/cas-supervisor/references/workflow.md");
        let claude_reference =
            include_str!("builtins/skills/cas-supervisor/references/reference.md");
        let codex_body = include_str!("builtins/codex/skills/cas-supervisor.md");
        let codex_model =
            include_str!("builtins/codex/skills/cas-supervisor/references/model-selection.md");
        let codex_workflow =
            include_str!("builtins/codex/skills/cas-supervisor/references/workflow.md");
        let codex_reference =
            include_str!("builtins/codex/skills/cas-supervisor/references/reference.md");
        let grok_body = include_str!("builtins/grok/skills/cas-supervisor.md");
        let grok_model =
            include_str!("builtins/grok/skills/cas-supervisor/references/model-selection.md");
        let grok_workflow =
            include_str!("builtins/grok/skills/cas-supervisor/references/workflow.md");
        let grok_reference =
            include_str!("builtins/grok/skills/cas-supervisor/references/reference.md");

        assert_eq!(
            claude_reference.replace("mcp__cas__", "mcp__cs__"),
            codex_reference,
            "Codex reference.md must normalize to the Claude twin"
        );
        assert_eq!(
            claude_reference.replace("mcp__cas__", "cas__"),
            grok_reference,
            "Grok reference.md must normalize to the Claude twin"
        );

        for (label, content) in [
            ("claude body", claude_body),
            ("claude model-selection", claude_model),
            ("claude workflow", claude_workflow),
            ("claude reference", claude_reference),
            ("codex body", codex_body),
            ("codex model-selection", codex_model),
            ("codex workflow", codex_workflow),
            ("codex reference", codex_reference),
            ("grok body", grok_body),
            ("grok model-selection", grok_model),
            ("grok workflow", grok_workflow),
            ("grok reference", grok_reference),
        ] {
            let lines: Vec<_> = content.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                if line.contains("coordination action=spawn_workers") {
                    for argument in ["cli=", "model=", "effort="] {
                        assert!(
                            line.contains(argument),
                            "{label}:{} spawn_workers recipe omits {argument}: {line:?}",
                            index + 1
                        );
                    }
                    if line.contains("cli=codex") {
                        assert!(
                            [
                                ("model=gpt-5.6-luna", "effort=xhigh"),
                                ("model=gpt-5.6-sol", "effort=high")
                            ]
                            .iter()
                                .any(|(model, effort)| line.contains(model) && line.contains(effort)),
                            "{label}:{} Codex spawn must use the Luna/Sol tier matrix: {line:?}",
                            index + 1
                        );
                    }
                    assert!(
                        !line.contains("model=gpt-5.5") && !line.contains("model=sonnet"),
                        "{label}:{} contains a disallowed worker route: {line:?}",
                        index + 1
                    );
                }

                if line.contains("coordination action=message") && line.contains("target=") {
                    let mut command = (*line).to_string();
                    let mut command_index = index;
                    while command.trim_end().ends_with('\\') && command_index + 1 < lines.len() {
                        command_index += 1;
                        command.push(' ');
                        command.push_str(lines[command_index].trim());
                    }
                    assert!(
                        command.contains("message=") && command.contains("summary="),
                        "{label}:{} coordination message example must include message= and summary=: {command:?}",
                        index + 1
                    );
                }
            }
        }
    }

    /// cas-1dbf: lessons from the codex-worker fix-round loop must stay in the
    /// supervisor reference layer, mirrored across Claude and Codex surfaces.
    #[test]
    fn test_supervisor_fix_round_recovery_guidance_present_and_mirrored() {
        for path in [
            "skills/cas-supervisor/references/code-review-queue.md",
            "skills/cas-supervisor/references/planning.md",
            "skills/cas-supervisor/references/worker-recovery.md",
            "skills/cas-supervisor/references/workflow.md",
        ] {
            let claude = BUILTIN_SKILLS
                .iter()
                .find(|b| b.path == path)
                .unwrap_or_else(|| panic!("BUILTIN_SKILLS missing {path}"));
            let codex = CODEX_BUILTIN_SKILLS
                .iter()
                .find(|b| b.path == path)
                .unwrap_or_else(|| panic!("CODEX_BUILTIN_SKILLS missing {path}"));
            // cas-2c61/cas-62ab: identical modulo the intentional per-harness
            // tool prefix — the codex copy correctly uses mcp__cs__, not
            // Claude's mcp__cas__.
            assert_eq!(
                claude.content.replace("mcp__cas__", "mcp__cs__"),
                codex.content,
                "{path} .claude and .codex copies must be identical apart from the \
                 mcp__cas__/mcp__cs__ tool prefix",
            );
        }

        let code_review_queue = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/code-review-queue.md")
            .expect("BUILTIN_SKILLS missing cas-supervisor code-review-queue.md");
        for required in [
            "not the full-review trigger",
            "Phase 3 uses a lightweight per-merge gate",
            "single required full `/cas-code-review` run happens in Phase 4",
            "create the task first",
            "epic-level review fix rounds",
            "messages are not durable task state",
        ] {
            assert!(
                code_review_queue.content.contains(required),
                "code-review-queue.md missing fix-round marker: {required:?}"
            );
        }

        let worker_recovery = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/worker-recovery.md")
            .expect("BUILTIN_SKILLS missing cas-supervisor worker-recovery.md");
        for required in [
            "Verify Lifecycle Notifications Before Acting",
            "cas-dbbe",
            "Injected but Unwoken Worker",
            "processed_at, acked_at",
            "urgent=true",
            "Do not kill or respawn",
        ] {
            assert!(
                worker_recovery.content.contains(required),
                "worker-recovery.md missing recovery marker: {required:?}"
            );
        }

        let workflow = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/workflow.md")
            .expect("BUILTIN_SKILLS missing cas-supervisor workflow.md");
        for required in [
            "Run the lightweight per-merge gate",
            "Do **not** run the full multi-persona",
            "Record the audit trail",
            "exception, not the default cadence",
            "Hold the main merge",
            "single required full multi-persona review",
            "git diff <base-branch>..HEAD > /tmp/<epic-id>-diff.patch",
            "bounded epic-child fix-round task",
            "cargo test --no-fail-fast > /tmp/<epic-id>-cargo-test.log 2>&1; echo $?",
            "Never pipe the test run to `tail`",
        ] {
            assert!(
                workflow.content.contains(required),
                "workflow.md missing epic-review marker: {required:?}"
            );
        }
        let phase3 = workflow
            .content
            .split("## Phase 4: Complete")
            .next()
            .expect("workflow.md must contain Phase 3 content before Phase 4");
        assert!(
            !phase3.contains("/cas-code-review mode=interactive base_sha=<pre_cp>"),
            "workflow.md Phase 3 must not mandate the old full review invocation"
        );

        let planning = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/references/planning.md")
            .expect("BUILTIN_SKILLS missing cas-supervisor planning.md");
        for required in [
            "Supervisors run a lightweight per-merge gate",
            "one full multi-persona `/cas-code-review` against the assembled EPIC diff",
        ] {
            assert!(
                planning.content.contains(required),
                "planning.md missing review-cadence marker: {required:?}"
            );
        }
    }

    /// MERGE REQUIRED was the single most frequent worker close rejection in
    /// downstream factory logs (gabber-studio, ozer) with zero skill guidance,
    /// and its friction normalized a verification-forging "dual-gate" bypass
    /// (`status=closed` + hand-written `verification action=add`). Pin the
    /// remediation guidance and the bypass ban on both surfaces so neither
    /// mirror silently drops them.
    #[test]
    fn test_worker_merge_state_guidance_present_and_mirrored() {
        for (label, set) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
            ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
        ] {
            for path in [
                "skills/cas-worker/references/close-gate.md",
                "skills/cas-worker/references/recovery.md",
            ] {
                let entry = set
                    .iter()
                    .find(|b| b.path == path)
                    .unwrap_or_else(|| panic!("{label} missing {path}"));
                for required in [
                    "MERGE REQUIRED",
                    "gh pr create",
                    "status=closed",
                    "inbox_poll",
                    "unread supervisor messages",
                    "git rev-parse factory/<name>",
                ] {
                    assert!(
                        entry.content.contains(required),
                        "{label} {path} missing merge-state guidance marker: {required:?}"
                    );
                }
            }
        }
        // recovery.md mirrors intentionally diverge by MCP alias (cas-5b4f):
        // the Codex copy's executable remediation must use the cs alias.
        let codex_recovery = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-worker/references/recovery.md")
            .expect("CODEX_BUILTIN_SKILLS missing recovery.md");
        assert!(
            codex_recovery
                .content
                .contains("mcp__cs__coordination action=message target=supervisor"),
            "codex recovery.md MERGE REQUIRED section must use the mcp__cs__ alias"
        );
        assert!(
            codex_recovery
                .content
                .contains("mcp__cs__coordination action=inbox_poll"),
            "codex recovery.md inbox remediation must use the mcp__cs__ alias"
        );
        // The SessionStart-injected body must surface the MERGE REQUIRED close
        // outcome and the literal-`supervisor` messaging target on both surfaces.
        for (label, guide) in [
            ("claude cas-worker.md", WORKER_GUIDE),
            (
                "codex cas-worker.md",
                include_str!("builtins/codex/skills/cas-worker.md"),
            ),
            (
                "grok cas-worker.md",
                include_str!("builtins/grok/skills/cas-worker.md"),
            ),
        ] {
            for required in [
                "MERGE REQUIRED",
                "literal string `supervisor`",
                "inbox_poll",
                "unread supervisor messages",
                "current factory-branch tip SHA",
            ] {
                assert!(
                    guide.contains(required),
                    "{label} missing worker-protocol marker: {required:?}"
                );
            }
        }
    }

    /// cas-e7c8: a haiku/low-tier worker (lt-defects, 2026-07-07) called
    /// `ToolSearch(select:mcp__cas__task)` seven times in a row and never
    /// once issued the follow-up `mcp__cas__task` call — it never
    /// distinguished "load the schema" from "call the tool". Pins the
    /// step-0 clarification in cas-worker.md and the matching recovery.md
    /// escape hatch on both mirrors so this guidance can't silently erode.
    #[test]
    fn test_worker_toolsearch_two_step_guidance_present_and_mirrored() {
        for (label, guide) in [
            ("claude cas-worker.md", WORKER_GUIDE),
            (
                "codex cas-worker.md",
                include_str!("builtins/codex/skills/cas-worker.md"),
            ),
        ] {
            for required in [
                "Tool loading is two steps, not one",
                "does **not** execute the tool",
                "not another ToolSearch",
            ] {
                assert!(
                    guide.contains(required),
                    "{label} missing ToolSearch two-step marker: {required:?}"
                );
            }
        }

        for (label, set) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let path = "skills/cas-worker/references/recovery.md";
            let entry = set
                .iter()
                .find(|b| b.path == path)
                .unwrap_or_else(|| panic!("{label} missing {path}"));
            for required in [
                "ToolSearch resolved the tool but you still can't call it",
                "Do not re-run ToolSearch for a tool it already resolved",
            ] {
                assert!(
                    entry.content.contains(required),
                    "{label} {path} missing ToolSearch-resolved recovery marker: {required:?}"
                );
            }
        }

        // recovery.md mirrors intentionally diverge by MCP alias (cas-5b4f) —
        // the new section must follow the same convention as the rest of the file.
        let claude_recovery = BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-worker/references/recovery.md")
            .expect("BUILTIN_SKILLS missing recovery.md");
        assert!(
            claude_recovery
                .content
                .contains("literally named `mcp__cas__task`"),
            "claude recovery.md ToolSearch section must use the mcp__cas__ alias"
        );
        let codex_recovery = CODEX_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-worker/references/recovery.md")
            .expect("CODEX_BUILTIN_SKILLS missing recovery.md");
        assert!(
            codex_recovery
                .content
                .contains("literally named `mcp__cs__task`"),
            "codex recovery.md ToolSearch section must use the mcp__cs__ alias"
        );
    }

    /// cas-3558: the 2026-07-09 grok run had an idle worker self-dispatch
    /// through the entire ready backlog ("session can exit. Starting
    /// cas-48e6…") with no supervisor assignment — the skill said "no
    /// grabbing unassigned tasks" but never spelled out that `action=ready`
    /// / `action=available` are visibility-only, and step 7 (close) never
    /// looped back to "go wait", so an idle worker filled the gap by
    /// self-serving. Pins the strengthened guidance across all three
    /// harness mirrors (Claude, Codex, Grok) so it can't silently erode.
    #[test]
    fn test_worker_never_self_dispatch_guidance_present_and_mirrored() {
        for (label, guide) in [
            ("claude cas-worker.md", WORKER_GUIDE),
            (
                "codex cas-worker.md",
                include_str!("builtins/codex/skills/cas-worker.md"),
            ),
            (
                "grok cas-worker.md",
                include_str!("builtins/grok/skills/cas-worker.md"),
            ),
        ] {
            for required in [
                "no self-dispatch",
                "This applies every time you go idle, not just at session start",
                "backlog *visibility*, never authorization to `start` a task yourself",
                "Never self-dispatch.",
                "Do not pull the next ready task yourself",
            ] {
                assert!(
                    guide.contains(required),
                    "{label} missing self-dispatch guard marker: {required:?}"
                );
            }
        }

        for (label, set) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
            ("GROK_BUILTIN_SKILLS", GROK_BUILTIN_SKILLS),
        ] {
            let path = "skills/cas-worker/references/details.md";
            let entry = set
                .iter()
                .find(|b| b.path == path)
                .unwrap_or_else(|| panic!("{label} missing {path}"));
            assert!(
                entry
                    .content
                    .contains("read-only backlog visibility — not self-dispatch"),
                "{label} {path} missing the ready/available visibility-only caveat"
            );
        }
    }

    // cas-e0d1: pin the opt-in description so a future sync or hand-edit can't
    // silently re-introduce auto-trigger phrasing into either mirror — that
    // would resurrect the wall-clock regression the rewrite fixed.
    #[test]
    fn test_cas_nuxt_playwright_description_is_opt_in() {
        for (label, set) in [
            ("BUILTIN_SKILLS", BUILTIN_SKILLS),
            ("CODEX_BUILTIN_SKILLS", CODEX_BUILTIN_SKILLS),
        ] {
            let entry = set
                .iter()
                .find(|b| b.path == "skills/cas-nuxt-playwright/SKILL.md")
                .unwrap_or_else(|| panic!("{label} missing cas-nuxt-playwright SKILL.md"));
            assert!(
                entry.content.contains("Opt-in only")
                    && entry
                        .content
                        .contains("invoke ONLY when the operator explicitly asks"),
                "{label}: cas-nuxt-playwright description must keep explicit opt-in wording"
            );
            assert!(
                !entry
                    .content
                    .contains("Trigger when editing files under tests/"),
                "{label}: cas-nuxt-playwright description must NOT re-introduce \
                 auto-trigger phrasing"
            );
        }
    }

    // cas-e0d1: the user-level prune must drop legacy non-managed cas-* orphans
    // (e.g. cas-playwright-debug) while preserving managed builtins and any
    // non-cas user skill. Covers all three guard branches plus idempotency.
    #[test]
    fn test_prune_stale_cas_skill_dirs_orphan_removed_managed_and_non_cas_kept() {
        use std::collections::HashSet;
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let skills_dir = temp.path().join("skills");

        let write_skill = |dir: &str, body: &str| {
            let p = skills_dir.join(dir);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("SKILL.md"), body).unwrap();
            p
        };

        // 1. Legacy non-managed cas-* orphan (no marker, not a builtin) — REMOVED.
        let orphan = write_skill(
            "cas-playwright-debug",
            "---\nname: cas-playwright-debug\nuser-invocable: true\n---\n# legacy\n",
        );
        // 2. Managed builtin carrying the marker but NOT in `keep` — preserved by
        //    the managed_by: cas marker guard.
        let managed = write_skill(
            "cas-nuxt-playwright",
            "---\nname: cas-nuxt-playwright\nmanaged_by: cas\n---\n# keep\n",
        );
        // 3. Builtin present in `keep` but missing the marker — preserved by the
        //    builtin-name guard.
        let kept_by_name = write_skill("cas-codemap", "---\nname: cas-codemap\n---\n# no marker\n");
        // 4. Non-cas user-authored skill — never touched.
        let non_cas = write_skill("my-skill", "---\nname: my-skill\n---\n# user\n");

        let mut keep = HashSet::new();
        keep.insert("cas-codemap".to_string());

        let removed = prune_stale_cas_skill_dirs(&skills_dir, &keep).unwrap();

        assert_eq!(removed, vec!["cas-playwright-debug".to_string()]);
        assert!(
            !orphan.exists(),
            "non-managed cas-* orphan should be removed"
        );
        assert!(
            managed.exists(),
            "managed_by: cas builtin should be preserved via marker guard"
        );
        assert!(
            kept_by_name.exists(),
            "builtin in keep set should be preserved via name guard"
        );
        assert!(non_cas.exists(), "non-cas dir should be untouched");

        // Idempotent: a second pass with nothing stale removes nothing.
        let removed2 = prune_stale_cas_skill_dirs(&skills_dir, &keep).unwrap();
        assert!(removed2.is_empty(), "second prune should be a no-op");
    }

    // cas-e0d1: builtin_skill_dir_names extracts `<dir>` from `skills/<dir>/...`
    // paths so the real builtin set protects those dirs from the prune.
    #[test]
    fn test_builtin_skill_dir_names_extracts_dirs_and_protects_nuxt_playwright() {
        let names = builtin_skill_dir_names(BUILTIN_SKILLS);
        assert!(
            names.contains("cas-nuxt-playwright"),
            "builtin skill dir set should contain cas-nuxt-playwright"
        );
        // The legacy orphan is NOT a builtin, so it is never in the keep set.
        assert!(
            !names.contains("cas-playwright-debug"),
            "cas-playwright-debug is not a builtin and must not be in the keep set"
        );
    }

    #[test]
    fn test_sync_all_codex_builtins_includes_agents() {
        // Verify sync_all_codex_builtins syncs agents (which includes task-verifier)
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let codex_dir = temp.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();

        let result = sync_all_codex_builtins(&codex_dir).unwrap();

        // Should sync at least 1 agent (task-verifier)
        assert!(
            result.agents_updated > 0,
            "sync_all_codex_builtins should sync agents"
        );

        // Verify task-verifier file was created
        let task_verifier_path = codex_dir.join("agents/task-verifier.md");
        assert!(
            task_verifier_path.exists(),
            "task-verifier.md should be created by sync_all_codex_builtins"
        );
    }

    /// cas-2c61: every Codex builtin (agent or skill) must reference the
    /// codex-aliased tool prefix `mcp__cs__` (per
    /// `SupervisorCli::Codex.backend().capabilities().tool_prefix`), never `mcp__cas__`
    /// (Claude's prefix). A codex worker/supervisor following a skill that
    /// carries the wrong prefix calls a tool name that doesn't resolve.
    /// Anti-drift guard mirroring the Grok corpus check (cas-6f46).
    #[test]
    fn test_codex_builtins_never_reference_claude_tool_prefix() {
        for builtin in CODEX_BUILTIN_SKILLS
            .iter()
            .chain(CODEX_BUILTIN_AGENTS.iter())
        {
            assert!(
                !builtin.content.contains("mcp__cas__"),
                "{} must not reference mcp__cas__ (Claude's prefix) — Codex uses mcp__cs__",
                builtin.path
            );
        }
    }

    // =========================================================================
    // EPIC cas-8888 (cas-6f46, Phase 5): Grok config wiring + skill twins
    // =========================================================================

    #[test]
    fn test_sync_all_grok_builtins_includes_agents_and_skills() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let grok_dir = temp.path().join(".grok");
        std::fs::create_dir_all(&grok_dir).unwrap();

        let result = sync_all_grok_builtins(&grok_dir).unwrap();

        assert!(
            result.agents_updated > 0,
            "sync_all_grok_builtins should sync agents"
        );
        assert!(
            result.skills_updated > 0,
            "sync_all_grok_builtins should sync skills"
        );

        assert!(
            grok_dir.join("agents/task-verifier.md").exists(),
            "task-verifier.md should be created by sync_all_grok_builtins"
        );
        assert!(
            grok_dir.join("skills/cas-worker/SKILL.md").exists(),
            "cas-worker/SKILL.md should be created by sync_all_grok_builtins"
        );
        assert!(
            grok_dir.join("skills/cas-supervisor/SKILL.md").exists(),
            "cas-supervisor/SKILL.md should be created by sync_all_grok_builtins"
        );
        assert!(
            grok_dir
                .join("skills/cas-supervisor-checklist/SKILL.md")
                .exists(),
            "cas-supervisor-checklist/SKILL.md should be created by sync_all_grok_builtins"
        );
    }

    #[test]
    fn test_sync_all_builtins_for_harness_routes_grok_to_grok_set() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let target = temp.path().join(".grok");
        std::fs::create_dir_all(&target).unwrap();

        let result = sync_all_builtins_for_harness(SupervisorCli::Grok, &target).unwrap();

        assert!(result.agents_updated > 0);
        assert!(result.skills_updated > 0);
        assert!(target.join("skills/cas-worker/SKILL.md").exists());
    }

    #[test]
    fn test_preview_all_builtins_for_harness_routes_grok_to_grok_set() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let target = temp.path().join(".grok");
        std::fs::create_dir_all(&target).unwrap();

        // Target is empty, so every builtin is a "new" change.
        let changes = preview_all_builtins_for_harness(SupervisorCli::Grok, &target).unwrap();

        assert!(
            !changes.is_empty(),
            "expected preview changes on an empty target"
        );
        assert!(changes.iter().all(|c| c.is_new));
        assert!(
            changes
                .iter()
                .any(|c| c.path == "skills/cas-worker/SKILL.md")
        );
    }

    #[test]
    fn test_prune_stale_user_skills_for_harness_uses_grok_skill_set() {
        use tempfile::tempdir;

        let temp = tempdir().unwrap();
        let grok_dir = temp.path().join(".grok");
        std::fs::create_dir_all(&grok_dir).unwrap();

        // Sync first so the real builtin dirs exist and are correctly kept...
        sync_all_grok_builtins(&grok_dir).unwrap();

        // ...then plant a stale, unmanaged cas-* orphan that isn't part of
        // GROK_BUILTIN_SKILLS and confirm it gets pruned, while a real
        // builtin dir (cas-worker) survives.
        let orphan_dir = grok_dir.join("skills").join("cas-orphan-skill");
        std::fs::create_dir_all(&orphan_dir).unwrap();
        std::fs::write(orphan_dir.join("SKILL.md"), "not managed by cas").unwrap();

        let removed = prune_stale_user_skills_for_harness(SupervisorCli::Grok, &grok_dir).unwrap();

        assert!(
            removed.contains(&"cas-orphan-skill".to_string()),
            "expected cas-orphan-skill to be pruned, got: {removed:?}"
        );
        assert!(
            grok_dir.join("skills/cas-worker").exists(),
            "a real Grok builtin skill dir must survive pruning"
        );
    }

    /// cas-6f46: every Grok skill twin must use the `cas__` tool prefix —
    /// never `mcp__cas__` (Claude) or `mcp__cs__` (Codex). A Grok worker
    /// copying tool-call syntax from a skill with the wrong prefix gets a
    /// tool-not-found error instead of a working call.
    #[test]
    fn test_grok_builtin_skills_never_reference_mcp_wrapped_tool_names() {
        for builtin in GROK_BUILTIN_SKILLS.iter().chain(GROK_BUILTIN_AGENTS.iter()) {
            assert!(
                !builtin.content.contains("mcp__cas__"),
                "{} must not reference mcp__cas__ (Claude's prefix) — Grok uses cas__",
                builtin.path
            );
            assert!(
                !builtin.content.contains("mcp__cs__"),
                "{} must not reference mcp__cs__ (Codex's prefix) — Grok uses cas__",
                builtin.path
            );
        }
    }

    /// cas-6f46 AC: "a grok worker following its cas-worker twin can call
    /// cas__task successfully" — the twin must actually reference the
    /// cas__ prefixed tool names a Grok worker needs for its core workflow.
    #[test]
    fn test_grok_worker_skill_references_cas_prefixed_tools() {
        let worker = GROK_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-worker/SKILL.md")
            .expect("GROK_BUILTIN_SKILLS missing cas-worker/SKILL.md");

        for required in ["cas__task", "cas__coordination"] {
            assert!(
                worker.content.contains(required),
                "grok cas-worker skill missing required tool reference: {required:?}"
            );
        }
    }

    /// cas-6f46: the Grok supervisor twin must carry the same deliberate
    /// model-tiering rule as the Claude (cas-c093) and Codex (cas-edf4)
    /// copies — the whole point of mirroring it a third time is to close
    /// this exact fleet-default footgun for every harness.
    #[test]
    fn test_grok_supervisor_skill_carries_model_tiering_rule() {
        let supervisor = GROK_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor/SKILL.md")
            .expect("GROK_BUILTIN_SKILLS missing cas-supervisor/SKILL.md");

        for keyword in [
            "Tier every spawn",
            "never fleet-default",
            "light",
            "standard",
            "heavy",
            "frontier",
            // cas-b342/cas-96ea: Codex-first tier table + exception/capacity lanes in the body.
            "Codex-first tiers",
            "grok/grok-composer-2.5-fast/low",
            "codex/gpt-5.6-sol/high",
            "codex/gpt-5.6-luna/xhigh",
            "Opus",
            "capacity route",
        ] {
            assert!(
                supervisor.content.contains(keyword),
                "grok cas-supervisor skill missing tiering-rule keyword: {keyword:?}"
            );
        }
    }

    /// cas-6f46: the Grok supervisor checklist must be modeled on the
    /// Claude version (real SessionStart hooks), not Codex's "no hooks"
    /// compensation variant — Grok's capability tier matches Claude's.
    #[test]
    fn test_grok_supervisor_checklist_is_not_the_no_hooks_variant() {
        let checklist = GROK_BUILTIN_SKILLS
            .iter()
            .find(|b| b.path == "skills/cas-supervisor-checklist/SKILL.md")
            .expect("GROK_BUILTIN_SKILLS missing cas-supervisor-checklist/SKILL.md");

        assert!(
            !checklist.content.to_lowercase().contains("no hooks"),
            "grok checklist must not carry Codex's no-hooks-compensation framing — \
             Grok has real SessionStart hooks like Claude"
        );
        assert!(
            !checklist.content.contains("Compensates for missing hooks"),
            "grok checklist description must not claim to compensate for missing hooks"
        );
    }

    /// cas-a326: the binary-freshness check runs through the very MCP server
    /// it may find stale. It must hand the restart to the harness owner, not
    /// strand the supervisor by teaching it to kill its own stdio server.
    #[test]
    fn test_supervisor_checklists_delegate_stale_serve_recovery_to_operator() {
        for (label, set, path) in [
            (
                "claude",
                BUILTIN_SKILLS,
                "skills/cas-supervisor-checklist/SKILL.md",
            ),
            (
                "codex",
                CODEX_BUILTIN_SKILLS,
                "skills/cas-codex-supervisor-checklist/SKILL.md",
            ),
            (
                "grok",
                GROK_BUILTIN_SKILLS,
                "skills/cas-supervisor-checklist/SKILL.md",
            ),
        ] {
            let checklist = set
                .iter()
                .find(|builtin| builtin.path == path)
                .unwrap_or_else(|| panic!("{label} missing {path}"));

            for required in [
                "do not kill or restart `cas serve` from this active MCP session",
                "ask the operator",
                "MCP reconnect/restart control",
                "Do not use `pkill`",
                "Cassy tool list is restored",
                "rerun this checklist from step 0",
            ] {
                assert!(
                    checklist.content.contains(required),
                    "{label} stale-binary recovery is missing {required:?}"
                );
            }
            assert!(
                !checklist
                    .content
                    .contains("restart any live `cas serve` processes before continuing"),
                "{label} checklist must not teach active-session self-restart"
            );
        }
    }

    // ----------------------------------------------------------------------
    // cas-cc8c: cross-harness required-capability parity (semantic).
    //
    // These normalize ONLY intentional differences — the twin-spelling per
    // harness in REQUIRED_FACTORY_CAPABILITIES, and the tool-prefix that each
    // harness legitimately uses — and FAIL on a genuine gap: a required
    // capability/agent missing from a harness's own catalog, a referenced
    // twin whose SKILL.md is absent, or a catalog leaking the wrong harness's
    // MCP tool prefix.
    // ----------------------------------------------------------------------

    /// Every required capability resolves to a present `SKILL.md` in each
    /// harness's OWN catalog (not by inheriting another harness's home). This is
    /// the load-bearing cas-cc8c assertion: adding cas-search/brainstorm/ideate
    /// to Grok is what makes this pass for `SupervisorCli::Grok`.
    #[test]
    fn test_required_capabilities_resolved_by_every_harness() {
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
            SupervisorCli::OpenCode,
        ] {
            let catalog = skill_catalog_for_harness(harness);
            for cap in REQUIRED_FACTORY_CAPABILITIES {
                let Some(dir) = required_dir_for(cap, harness) else {
                    continue; // intentional exemption (guarded by the note test)
                };
                let skill_md = format!("{dir}/SKILL.md");
                assert!(
                    catalog.iter().any(|b| b.path == skill_md),
                    "{harness:?} catalog is missing required capability '{}' \
                     (expected twin at {skill_md}) — a factory {harness:?} session \
                     must resolve it from its own catalog, not by inheriting \
                     another harness's home directory",
                    cap.id
                );
            }
        }
    }

    /// cas-20f2: every GENERAL-parity skill also resolves to a present `SKILL.md`
    /// in each harness's own catalog — the operator-requested full parity. Grok
    /// owns twins for session-learn, codemap, project-overview, fallow,
    /// cas-nuxt-playwright, and cas-codex-exec rather than inheriting `~/.claude`.
    #[test]
    fn test_general_parity_capabilities_resolved_by_every_harness() {
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
            SupervisorCli::OpenCode,
        ] {
            let catalog = skill_catalog_for_harness(harness);
            for cap in GENERAL_PARITY_CAPABILITIES {
                let Some(dir) = required_dir_for(cap, harness) else {
                    continue; // documented runtime-prerequisite exemption
                };
                let skill_md = format!("{dir}/SKILL.md");
                assert!(
                    catalog.iter().any(|b| b.path == skill_md),
                    "{harness:?} catalog is missing general-parity skill '{}' \
                     (expected twin at {skill_md}) — full parity requires it to \
                     resolve from {harness:?}'s own catalog, not from ~/.claude",
                    cap.id
                );
            }
        }
    }

    /// A capability may be exempt for a harness (`None`) only when it documents
    /// why. Prevents silently dropping a required capability by nulling a field.
    #[test]
    fn test_required_capability_exemptions_are_documented() {
        for cap in REQUIRED_FACTORY_CAPABILITIES
            .iter()
            .chain(GENERAL_PARITY_CAPABILITIES.iter())
        {
            let has_none = cap.claude.is_none() || cap.codex.is_none() || cap.grok.is_none();
            if has_none {
                assert!(
                    !cap.note.trim().is_empty(),
                    "required capability '{}' exempts a harness (None) without a \
                     documented reason in `note`",
                    cap.id
                );
            }
        }
    }

    /// Every required twin actually referenced by the manifest points at a real,
    /// non-empty, `managed_by: cas` skill file — so "install missing" installs a
    /// working skill and the overwrite gate will keep it fresh.
    #[test]
    fn test_required_capability_twins_are_managed_and_nonempty() {
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
            SupervisorCli::OpenCode,
        ] {
            let catalog = skill_catalog_for_harness(harness);
            for cap in REQUIRED_FACTORY_CAPABILITIES {
                let Some(dir) = required_dir_for(cap, harness) else {
                    continue;
                };
                let skill_md = format!("{dir}/SKILL.md");
                let file = catalog
                    .iter()
                    .find(|b| b.path == skill_md)
                    .unwrap_or_else(|| panic!("{harness:?} missing {skill_md}"));
                assert!(
                    file.content.len() > 40,
                    "{harness:?} {skill_md} looks empty/stub"
                );
                assert!(
                    is_managed_by_cas(file.content),
                    "{harness:?} {skill_md} must carry `managed_by: cas` so sync can \
                     install/overwrite it"
                );
            }
        }
    }

    /// Required agent roles have equivalent coverage across all four harnesses.
    /// Harness-specific extras (Codex `factory-supervisor`) are allowed and are
    /// simply not in the required set.
    #[test]
    fn test_required_agents_present_in_every_harness() {
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
            SupervisorCli::OpenCode,
        ] {
            let catalog = agent_catalog_for_harness(harness);
            for agent in REQUIRED_FACTORY_AGENTS {
                assert!(
                    catalog.iter().any(|b| &b.path == agent),
                    "{harness:?} agent catalog is missing required role {agent}"
                );
            }
        }
    }

    /// No tailored-harness catalog leaks a foreign MCP tool prefix in its OWN
    /// tool-call guidance (cas-cc8c AC-5). Grok must never carry `mcp__cas__`
    /// (Claude) or `mcp__cs__` (Codex); Codex must never carry `mcp__cas__`.
    /// (Claude is the reference harness and legitimately documents the other
    /// aliases in cross-harness recovery guidance, so it is not swept here — its
    /// own tool calls use `mcp__cas__` by construction.) This spans every entry
    /// in both tailored catalogs, so the new cas-cc8c Grok required twins are
    /// covered automatically.
    #[test]
    fn test_tailored_catalogs_never_leak_foreign_tool_prefix() {
        for b in GROK_BUILTIN_SKILLS.iter().chain(GROK_BUILTIN_AGENTS.iter()) {
            assert!(
                !b.content.contains("mcp__cas__"),
                "Grok {} leaks Claude prefix mcp__cas__",
                b.path
            );
            assert!(
                !b.content.contains("mcp__cs__"),
                "Grok {} leaks Codex prefix mcp__cs__",
                b.path
            );
        }
        for b in CODEX_BUILTIN_SKILLS
            .iter()
            .chain(CODEX_BUILTIN_AGENTS.iter())
        {
            assert!(
                !b.content.contains("mcp__cas__"),
                "Codex {} leaks Claude prefix mcp__cas__",
                b.path
            );
        }
        for b in opencode_builtin_skills()
            .iter()
            .chain(opencode_builtin_agents().iter())
        {
            assert!(
                !b.content.contains("mcp__cas__")
                    && !b.content.contains("mcp__cs__")
                    && !b.content.contains("cas__"),
                "OpenCode {} leaks a foreign MCP prefix",
                b.path
            );
        }
    }

    #[test]
    fn test_opencode_projection_has_required_catalog_and_cas_tool_names() {
        let skills = skill_catalog_for_harness(SupervisorCli::OpenCode);
        let agents = agent_catalog_for_harness(SupervisorCli::OpenCode);
        assert_eq!(skills.len(), BUILTIN_SKILLS.len());
        assert_eq!(agents.len(), BUILTIN_AGENTS.len());
        assert!(skills.iter().any(|b| {
            b.path == "skills/cas-task-tracking/SKILL.md" && b.content.contains("cas_task")
        }));
        assert!(agents.iter().any(|b| b.path == "agents/task-verifier.md"));
        assert!(required_dir_for(
            &REQUIRED_FACTORY_CAPABILITIES[0],
            SupervisorCli::OpenCode
        )
        .is_some());
    }

    /// Demo / AC-2 end-to-end: a fresh sync of EACH harness into an empty temp
    /// tree installs every required-capability twin on disk — proving each
    /// harness resolves the required factory checklist from its OWN directory,
    /// with no other harness's home present. This is the executable form of the
    /// task's demo ("initialize each harness and print the same required factory
    /// capability checklist as PASS for Claude, Codex, and Grok").
    #[test]
    fn test_fresh_sync_installs_required_capabilities_for_every_harness() {
        use tempfile::tempdir;

        // OpenCode's catalog is projected into its generated primary-agent
        // config; it intentionally has no user-level skill tree to sync.
        for harness in [
            SupervisorCli::Claude,
            SupervisorCli::Codex,
            SupervisorCli::Grok,
        ] {
            let temp = tempdir().unwrap();
            let dir = temp.path().join("harness-home");
            std::fs::create_dir_all(&dir).unwrap();

            sync_all_builtins_for_harness(harness, &dir).unwrap();

            // Every required-factory AND general-parity capability must land on
            // disk from this harness's own catalog — no ~/.claude present.
            for cap in REQUIRED_FACTORY_CAPABILITIES
                .iter()
                .chain(GENERAL_PARITY_CAPABILITIES.iter())
            {
                let Some(rel) = required_dir_for(cap, harness) else {
                    continue;
                };
                let on_disk = dir.join(rel).join("SKILL.md");
                assert!(
                    on_disk.exists(),
                    "{harness:?} fresh sync did not install capability \
                     '{}' at {} — the checklist would FAIL for {harness:?}",
                    cap.id,
                    on_disk.display()
                );
            }
            for agent in REQUIRED_FACTORY_AGENTS {
                assert!(
                    dir.join(agent).exists(),
                    "{harness:?} fresh sync did not install required agent {agent}"
                );
            }
        }
    }

    /// The three new Grok required twins (cas-cc8c) exist and use the `cas__`
    /// prefix for the tools their workflow calls — a Grok session copying tool
    /// syntax from them must get a working call.
    #[test]
    fn test_grok_search_brainstorm_ideate_use_cas_prefix() {
        let expect = [
            ("skills/cas-search/SKILL.md", "cas__search"),
            ("skills/cas-brainstorm/SKILL.md", "cas__"),
            ("skills/cas-ideate/SKILL.md", "cas__"),
        ];
        for (path, needle) in expect {
            let file = GROK_BUILTIN_SKILLS
                .iter()
                .find(|b| b.path == path)
                .unwrap_or_else(|| panic!("GROK_BUILTIN_SKILLS missing {path}"));
            assert!(
                file.content.contains(needle),
                "grok {path} must reference {needle} (cas__ prefix)"
            );
        }
    }
}
