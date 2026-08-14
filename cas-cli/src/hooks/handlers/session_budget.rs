//! Aggregate size budget for the assembled SessionStart `additionalContext`
//! (cas-b114).
//!
//! Claude Code refuses to inline a hook payload past roughly 10KB: it persists
//! the text to a `tool-results/*.txt` file and shows the session a stub plus a
//! ~2KB preview. The user reads that as "the session start did nothing", and
//! the model silently loses everything past the preview unless it thinks to
//! read the file.
//!
//! `test_supervisor_guidance_under_8kb` budgets one *component* (the supervisor
//! skill body). Nothing budgeted the **assembled** payload, which also carries
//! the codemap-freshness warning, prior-factory WIP triage, orphan/GC leftovers,
//! GitHub issue triage, memories and ready tasks. Observed 11.9KB in a real
//! session on 2026-08-06.
//!
//! This module enforces the aggregate bound with **deterministic degradation**,
//! never blind truncation:
//!
//! * Protected segments (role guidance + the CAS context header, plus safety
//!   assertions such as the worker worktree warning) are emitted verbatim,
//!   always, whatever the budget says.
//! * Degradable segments carry a pre-authored *compact summary* — counts plus
//!   the remediation command — that replaces the full rendering when the
//!   payload is over budget.
//! * If compacting every degradable segment still leaves the payload over
//!   budget (i.e. the protected content alone exceeds it), degradable segments
//!   are dropped entirely rather than cut mid-sentence. Their information is
//!   always one named command away.
//!
//! Degradation order is deterministic: largest saving first, ties broken by
//! assembly order. Same inputs always produce the same payload.

/// Aggregate byte budget for the assembled SessionStart `additionalContext`.
///
/// 9KB leaves ~1KB of headroom under the harness' observed ~10KB inline cap so
/// that JSON escaping and the harness' own wrapper cannot push a payload that
/// passed this gate over the real limit.
pub(crate) const SESSION_START_BUDGET_BYTES: usize = 9 * 1024;

/// Separator between assembled segments (matches the pre-budget assembly).
const SEP: &str = "\n";

/// Base-context sections that may degrade to "heading + how to get it back".
///
/// Each entry is `(heading prefix, remediation)`. These are progressive-
/// disclosure *listings* — the heading itself already states the counts, and
/// every item is retrievable on demand — so summarising them costs the session
/// nothing, whereas losing role guidance to harness truncation costs it
/// everything. Anything not listed here is protected.
const DEGRADABLE_BASE_SECTIONS: &[(&str, &str)] = &[
    ("## Ready Tasks", "task action=ready"),
    ("## In Progress", "task action=mine"),
    ("## Helpful Memories", "memory action=recent"),
    ("## Related to Current Work", "search action=context"),
    ("## Available Skills", "skill action=list"),
    ("## Connected MCP Tools", "system action=status"),
];

/// Split the base context into budget segments at its `## ` headings.
///
/// Returns `(full, compact)` pairs in document order; `compact` is `None` for
/// protected sections. Splitting is lossless: joining the full texts with
/// [`SEP`] reproduces the input exactly.
fn split_base_context(base: &str) -> Vec<(String, Option<String>)> {
    if base.is_empty() {
        return Vec::new();
    }
    let prefix = crate::harness_policy::own_tool_prefix();
    let mut segments: Vec<(String, Option<String>)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_compact: Option<String> = None;

    let flush = |segments: &mut Vec<(String, Option<String>)>,
                 current: &mut Vec<&str>,
                 compact: &mut Option<String>| {
        if !current.is_empty() {
            segments.push((current.join(SEP), compact.take()));
            current.clear();
        }
    };

    for line in base.split(SEP) {
        let heading = line.starts_with("## ");
        if heading {
            flush(&mut segments, &mut current, &mut current_compact);
            current_compact = DEGRADABLE_BASE_SECTIONS
                .iter()
                .find(|(name, _)| line.starts_with(name))
                .map(|(_, remediation)| {
                    format!("{line}\n(omitted to fit the session-start size budget — run `{prefix}{remediation}`)")
                });
        }
        current.push(line);
    }
    flush(&mut segments, &mut current, &mut current_compact);
    segments
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Full,
    Compact,
    Dropped,
}

#[derive(Debug, Clone)]
struct Segment {
    full: String,
    /// `None` marks the segment protected — it is never compacted or dropped.
    compact: Option<String>,
    level: Level,
}

impl Segment {
    fn rendered(&self) -> &str {
        match self.level {
            Level::Full => &self.full,
            Level::Compact => self.compact.as_deref().unwrap_or(&self.full),
            Level::Dropped => "",
        }
    }

    fn is_degradable(&self) -> bool {
        self.compact.is_some()
    }
}

/// Ordered assembler for the SessionStart payload.
///
/// Segments keep the order they are added in; `prepend_*` pushes to the front
/// (used by warnings engineered to land inside the harness' preview window).
#[derive(Debug, Default)]
pub(crate) struct SessionContextAssembler {
    segments: Vec<Segment>,
    budget: Option<usize>,
}

impl SessionContextAssembler {
    /// Start from the base context (CAS header + role guidance + ready tasks +
    /// memories + skills + MCP tools).
    ///
    /// The base is split on its top-level section headings: the CAS context
    /// header and role guidance are protected verbatim, while the *listing*
    /// sections ([`DEGRADABLE_BASE_SECTIONS`]) degrade to their heading — which
    /// already carries the counts — plus the command that reproduces them. Any
    /// heading not on that list is protected, so a renamed or new section fails
    /// safe (kept in full) rather than being silently summarised.
    pub(crate) fn new(base: String) -> Self {
        let mut assembler = Self {
            segments: Vec::new(),
            budget: None,
        };
        for (full, compact) in split_base_context(&base) {
            assembler.push(false, full, compact);
        }
        assembler
    }

    /// Override the byte budget for another bounded delivery surface.
    /// Production SessionStart uses [`SESSION_START_BUDGET_BYTES`].
    pub(crate) fn with_budget(mut self, budget: usize) -> Self {
        self.budget = Some(budget);
        self
    }

    fn push(&mut self, front: bool, full: String, compact: Option<String>) {
        if full.trim().is_empty() {
            return;
        }
        let segment = Segment {
            full,
            compact: compact.filter(|c| !c.trim().is_empty()),
            level: Level::Full,
        };
        if front {
            self.segments.insert(0, segment);
        } else {
            self.segments.push(segment);
        }
    }

    /// Append a segment that must survive verbatim.
    pub(crate) fn append_protected(&mut self, text: String) {
        self.push(false, text, None);
    }

    /// Append a segment that degrades to `compact` when over budget.
    pub(crate) fn append_degradable(&mut self, full: String, compact: String) {
        self.push(false, full, Some(compact));
    }

    /// Prepend a segment that degrades to `compact` when over budget.
    pub(crate) fn prepend_degradable(&mut self, full: String, compact: String) {
        self.push(true, full, Some(compact));
    }

    fn joined(&self) -> String {
        self.segments
            .iter()
            .map(Segment::rendered)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(SEP)
    }

    /// Degradation order: biggest saving first, ties broken by assembly order.
    fn degradation_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_degradable())
            .map(|(i, _)| i)
            .collect();
        order.sort_by(|a, b| {
            let saving = |i: usize| {
                let seg = &self.segments[i];
                seg.full
                    .len()
                    .saturating_sub(seg.compact.as_deref().map(str::len).unwrap_or(0))
            };
            saving(*b).cmp(&saving(*a)).then(a.cmp(b))
        });
        order
    }

    /// Render the payload, degrading variable sections until it fits.
    ///
    /// Returns the assembled string. When protected content alone exceeds the
    /// budget the result can still be over — by design: guidance and the CAS
    /// context header are never truncated. A diagnostic is written to stderr in
    /// that case so the overflow is attributable instead of mysterious.
    pub(crate) fn render(mut self) -> String {
        let budget = self.budget.unwrap_or(SESSION_START_BUDGET_BYTES);
        let mut out = self.joined();
        if out.len() <= budget {
            return out;
        }

        let order = self.degradation_order();
        let mut compacted = 0usize;
        for &idx in &order {
            self.segments[idx].level = Level::Compact;
            compacted += 1;
            out = self.joined();
            if out.len() <= budget {
                eprintln!(
                    "cas: SessionStart payload over {budget}B budget — compacted {compacted} \
                     section(s) to summaries ({} bytes)",
                    out.len()
                );
                return out;
            }
        }

        let mut dropped = 0usize;
        for &idx in &order {
            self.segments[idx].level = Level::Dropped;
            dropped += 1;
            out = self.joined();
            if out.len() <= budget {
                eprintln!(
                    "cas: SessionStart payload over {budget}B budget — compacted {compacted} \
                     and dropped {dropped} section(s) ({} bytes)",
                    out.len()
                );
                return out;
            }
        }

        eprintln!(
            "cas: SessionStart payload is {} bytes with every degradable section dropped — \
             protected guidance/context alone exceeds the {budget}B budget. Trim role guidance \
             (see test_supervisor_guidance_under_8kb) rather than truncating it here.",
            out.len()
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(n: usize, ch: char) -> String {
        std::iter::repeat_n(ch, n).collect()
    }

    #[test]
    fn under_budget_payload_is_untouched() {
        let mut a = SessionContextAssembler::new("base".to_string()).with_budget(1000);
        a.append_degradable("full banner".to_string(), "compact".to_string());
        assert_eq!(a.render(), "base\nfull banner");
    }

    #[test]
    fn over_budget_degrades_largest_saver_first() {
        let mut a = SessionContextAssembler::new(seg(100, 'b')).with_budget(200);
        a.append_degradable(seg(50, 'x'), "x-compact".to_string());
        a.append_degradable(seg(200, 'y'), "y-compact".to_string());
        let out = a.render();
        assert!(out.len() <= 200, "payload {} bytes", out.len());
        // The bigger saver (y) compacts first; x keeps its full rendering.
        assert!(out.contains(&seg(50, 'x')), "small section kept full");
        assert!(out.contains("y-compact"), "large section compacted");
        assert!(!out.contains(&seg(200, 'y')));
    }

    #[test]
    fn protected_segments_are_never_truncated() {
        let base = seg(500, 'b');
        let mut a = SessionContextAssembler::new(base.clone()).with_budget(100);
        a.append_degradable(seg(300, 'y'), "y-compact".to_string());
        let out = a.render();
        assert!(out.contains(&base), "protected base must survive verbatim");
        // Degradable content is dropped, not cut mid-sentence.
        assert!(!out.contains("y-compact"));
        assert_eq!(out, base);
    }

    #[test]
    fn degradation_is_deterministic() {
        let build = || {
            let mut a = SessionContextAssembler::new(seg(100, 'b')).with_budget(220);
            a.append_degradable(seg(80, 'x'), "x-compact".to_string());
            a.append_degradable(seg(80, 'y'), "y-compact".to_string());
            a.render()
        };
        assert_eq!(build(), build());
        // Equal savings → assembly order decides: x compacts before y.
        let out = build();
        assert!(out.contains("x-compact"));
        assert!(out.contains(&seg(80, 'y')));
    }

    #[test]
    fn prepended_segments_lead_the_payload() {
        let mut a = SessionContextAssembler::new("base".to_string()).with_budget(1000);
        a.prepend_degradable("URGENT".to_string(), "urgent".to_string());
        assert!(a.render().starts_with("URGENT\nbase"));
    }

    /// The regression this module exists for (cas-b114): assemble the payload
    /// out of the REAL renderers, every variable section at its worst case, and
    /// assert the total stays under the harness' inline cap.
    ///
    /// Worst case here means: real supervisor guidance (the largest protected
    /// component, budgeted at ≤8000B by `test_supervisor_guidance_under_8kb`),
    /// a 198-change codemap staleness with long paths, a 240-file prior-factory
    /// WIP tree, and a full GitHub issue-triage list. Before the budget these
    /// summed to ~12KB and the harness silently filed the payload away.
    #[test]
    fn assembled_worst_case_payload_stays_under_the_inline_cap() {
        use crate::hooks::handlers::handlers_events::codemap::CodemapStaleness;
        use crate::hooks::handlers::session_hygiene::{
            PorcelainEntry, WipSummary, render_issues_target_banner, render_unfiled_reports_banner,
            render_wip_banner,
        };

        // Base: CAS header + real supervisor guidance (protected) followed by
        // the progressive-disclosure listings (degradable).
        let protected_base = format!(
            "## 📋 CAS Context\n**Session:** `7d3511aa-9cf5-44d8-921d-0289bd66fe0a`\n\n{}",
            crate::builtins::supervisor_guidance()
        );
        let listings = format!(
            "\n\n## Ready Tasks (5/5 shown, ~100tk if all expanded)\n{}\n\
             ## Helpful Memories (5 memories, ~1.4k tk if expanded)\n{}\n\
             ## Related to Current Work (5 items, ~1.4k tk if expanded)\n{}\n\
             ## Connected MCP Tools (4 servers, 92 tools)\n{}",
            (0..5)
                .map(|i| format!("- **cas-a1b{i}** A ready task with a reasonably long title {i}\n"))
                .collect::<String>(),
            (0..5)
                .map(|i| format!("- memory {i}: a learning captured in a previous session\n"))
                .collect::<String>(),
            (0..5)
                .map(|i| format!("- related item {i}: a file or task near the current work\n"))
                .collect::<String>(),
            (0..30)
                .map(|i| format!("- `mcp__server__tool_{i}` — does something useful\n"))
                .collect::<String>(),
        );
        let base = format!("{protected_base}{listings}");
        let mut assembler = SessionContextAssembler::new(base.clone());

        // Worst-case codemap: 198 structural changes, 10 long paths enumerated.
        let staleness = CodemapStaleness::SignificantlyStale {
            total_changes: 198,
            file_list: (0..10)
                .map(|i| {
                    format!("+cas-cli/src/builtins/codex/skills/cas-github-issues/SKILL-{i}.md")
                })
                .collect(),
            commit_info: " since commit a1b2c3d4 (2026-08-06)".to_string(),
        };
        assembler.prepend_degradable(
            staleness.format_injection(true),
            staleness.format_injection_compact(true),
        );

        // Worst-case prior-factory WIP: 240 dirty files (banner caps its own
        // inline rows at 20, but that is still ~2KB of payload).
        let wip = WipSummary {
            worktree: std::path::PathBuf::from("/home/agent/project"),
            entries: (0..240)
                .map(|i| PorcelainEntry {
                    status: if i % 2 == 0 { "??" } else { " M" }.to_string(),
                    path: format!("crates/cas-cli/src/hooks/handlers/generated_module_{i}.rs"),
                })
                .collect(),
        };
        let wip_banner = render_wip_banner(&wip);
        assembler.append_degradable(wip_banner.full, wip_banner.compact);

        // Worst-case orphan/GC leftovers and issue triage, rendered at the same
        // shape their modules emit (counts + rows, degrading to counts + command).
        let orphan_full = (0..10)
            .map(|i| format!("  [process] pid {i} (node) — reapable\n"))
            .collect::<String>();
        assembler.append_degradable(
            format!(
                "⚠ Leftovers from earlier sessions: 34 orphan process(es) in worktrees, \
                 6 stale server registration(s) — holding port(s) 3000, 3001, 5173.\n{orphan_full}",
            ),
            "⚠ Leftovers from earlier sessions: 34 orphan process(es), 6 stale server \
             registration(s) — run `mcp__cas__coordination action=gc_report`.\n"
                .to_string(),
        );
        let issues_full = (0..20)
            .map(|i| format!("\n- #{i} A fairly long GitHub issue title describing a defect {i}"))
            .collect::<String>();
        assembler.append_degradable(
            format!("## GitHub issue triage — owner/repo\n41 open (checked just now){issues_full}"),
            "## GitHub issue triage — owner/repo\n41 open — run `gh issue list --repo owner/repo`."
                .to_string(),
        );

        // cas-20f27 issue-filing detectors: a long unfiled-report backlog (the
        // real case that motivated them was 13 staged files) plus the fixed
        // unset-issues-target line.
        let staged: Vec<String> = (0..30)
            .map(|i| format!("BUG-a-fairly-long-descriptive-report-slug-{i:02}.md"))
            .collect();
        let unfiled_banner = render_unfiled_reports_banner(&staged);
        assembler.append_degradable(unfiled_banner.full, unfiled_banner.compact);
        let issues_target = render_issues_target_banner();
        assembler.append_degradable(issues_target.full, issues_target.compact);

        let payload = assembler.render();
        assert!(
            payload.len() <= SESSION_START_BUDGET_BYTES,
            "assembled SessionStart payload is {} bytes — over the {SESSION_START_BUDGET_BYTES}B \
             budget; the harness will file it to disk and show the session a 2KB preview",
            payload.len()
        );

        // Degradation, not truncation: protected guidance survives verbatim and
        // every compacted section still names its remediation command.
        assert!(
            payload.contains(&protected_base),
            "supervisor guidance / CAS context header must never be truncated"
        );
        assert!(
            payload.contains("198 structural change"),
            "codemap staleness must survive as counts + command, full or compact"
        );
        assert!(payload.contains("/codemap"));
        assert!(payload.contains("gc_report"));
        // cas-20f27: both filing detectors must survive degradation carrying
        // their remediation, and neither may be cut mid-sentence.
        assert!(
            payload.contains("30 staged bug/feature report(s)")
                && payload.contains("cas-github-issues"),
            "the unfiled-reports detector must survive as a count + its remediation skill"
        );
        assert!(
            payload.contains("cas config set issues.repo owner/repo"),
            "the unset-issues-target detector must survive with its exact command"
        );
        for line in payload.lines() {
            if line.contains("staged bug/feature report(s)") || line.contains("`[issues] repo`") {
                assert!(
                    line.trim_end().ends_with('.') || line.trim_end().ends_with(':'),
                    "degraded detector line must end as a complete sentence, got: {line}"
                );
            }
        }
    }

    #[test]
    fn base_split_is_lossless_and_only_marks_listing_sections_degradable() {
        let base = "## 📋 CAS Context\nheader line\n\n# Factory Supervisor\n\
                    ## Hard Rules\nnever break these\n\n\
                    ## Ready Tasks (5/5 shown)\n- cas-1234 do the thing\n";
        let segments = split_base_context(base);
        assert_eq!(
            segments
                .iter()
                .map(|(full, _)| full.clone())
                .collect::<Vec<_>>()
                .join(SEP),
            base,
            "splitting must be lossless"
        );
        let degradable: Vec<&str> = segments
            .iter()
            .filter(|(_, compact)| compact.is_some())
            .map(|(full, _)| full.as_str())
            .collect();
        assert_eq!(degradable.len(), 1, "only the listing section degrades");
        assert!(degradable[0].starts_with("## Ready Tasks"));
        // Guidance sections stay protected even though they are the largest.
        assert!(
            segments
                .iter()
                .any(|(full, compact)| full.starts_with("## Hard Rules") && compact.is_none())
        );
    }

    /// The compact form of a listing keeps the counts (they live in the
    /// heading) and names the command that brings the detail back.
    #[test]
    fn compacted_listings_keep_counts_and_name_the_command() {
        let base = "## 📋 CAS Context\nheader\n\n## Ready Tasks (5/5 shown, ~100tk)\n"
            .to_string()
            + &(0..40)
                .map(|i| format!("- cas-{i:04} a ready task with a long-ish title\n"))
                .collect::<String>();
        let payload = SessionContextAssembler::new(base)
            .with_budget(200)
            .render();
        assert!(payload.contains("## Ready Tasks (5/5 shown, ~100tk)"));
        assert!(payload.contains("action=ready"));
        assert!(!payload.contains("cas-0007"));
        assert!(payload.contains("header"), "protected header survives");
    }

    #[test]
    fn empty_segments_are_skipped() {
        let mut a = SessionContextAssembler::new(String::new()).with_budget(1000);
        a.append_protected("only".to_string());
        a.append_degradable(String::new(), "unused".to_string());
        assert_eq!(a.render(), "only");
    }
}
