//! The result of a project sweep as findings (cas-0aa5).
//!
//! Each failed, warned or planned phase becomes one row: a mark, who it is
//! about, the phase, the cause in its own words, and one remedy under `→`.
//! Identical findings across projects collapse to one row with a count and
//! the names beneath it. Healthy projects collapse to one counted line. The
//! raw output each phase printed stays under `--verbose`.

use std::io;
use std::path::{Path, PathBuf};

use super::{ProjectPhase, ProjectRefreshReceipt, SkippedProject};
use crate::cli::hub::HubRestartOutcome;
use crate::ui::components::{Formatter, Verdict};
use crate::ui::theme::Icons;

/// Prose never widens past this, however wide the terminal is.
const MAX_CELLS: usize = 100;
const WHO_CELLS: usize = 24;
const PHASE_CELLS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finding {
    pub(super) verdict: Verdict,
    pub(super) who: Vec<String>,
    pub(super) phase: &'static str,
    pub(super) cause: String,
    pub(super) remedy: Option<String>,
}

/// What a sweep found, in reading order: failures, warnings, notes.
pub(super) struct SweepFindings {
    pub(super) findings: Vec<Finding>,
    pub(super) healthy: Vec<String>,
    /// Healthy projects with no Cloud link, said once on the healthy line.
    pub(super) unlinked: usize,
    pub(super) user_level_current: bool,
    pub(super) project_count: usize,
    pub(super) config: ConfigCleanup,
}

/// Config files, including host state, are counted separately from projects.
#[derive(Default, serde::Serialize)]
pub(super) struct ConfigCleanup {
    pub(super) repaired: usize,
    pub(super) unchanged: usize,
    pub(super) skipped: usize,
    pub(super) failed: usize,
    pub(super) planned: usize,
    pub(super) entries_removed: usize,
}

impl ConfigCleanup {
    pub(super) fn include(&mut self, receipt: &crate::config::ConfigRetirement) {
        match receipt.status {
            "repaired" => {
                self.repaired += 1;
                self.entries_removed += receipt.removed.len();
            }
            "unchanged" => self.unchanged += 1,
            "missing" => self.skipped += 1,
            "failed" => self.failed += 1,
            "planned" => self.planned += 1,
            _ => {}
        }
    }
}

const PHASES: [&str; 5] = ["migr", "index", "skills", "member", "cloud"];

/// The phase names a detail may repeat as its own prefix ("skills: …").
const PHASE_PREFIXES: [&str; 7] = [
    "migration: ",
    "search index: ",
    "search_index: ",
    "skills: ",
    "membership: ",
    "cloud sync: ",
    "cloud: ",
];

pub(super) fn collect(
    receipts: &[ProjectRefreshReceipt],
    user_level: &ProjectPhase,
    skipped: &[SkippedProject],
    hub: Option<&HubRestartOutcome>,
    hub_transport_error: Option<&str>,
) -> SweepFindings {
    let names = display_names(receipts.iter().map(|receipt| receipt.project.as_path()));
    let mut findings: Vec<Finding> = Vec::new();
    let mut healthy = Vec::new();
    let mut unlinked = 0;
    let mut config = ConfigCleanup::default();
    for (receipt, name) in receipts.iter().zip(&names) {
        if let Some(maintenance) = &receipt.retired_toml {
            config.include(maintenance);
        }
        let phases = [
            &receipt.migration,
            &receipt.search_index,
            &receipt.skills,
            &receipt.membership,
            &receipt.cloud,
        ];
        let mut found = false;
        for (index, phase) in phases.into_iter().enumerate() {
            let output = receipt
                .phase_details
                .get(index)
                .map(|(_, output)| output.as_str())
                .unwrap_or_default();
            let verdict = match phase {
                ProjectPhase::Failed(_) => Verdict::Error,
                ProjectPhase::Warning(_) => Verdict::Warning,
                // A store the migration could not bring current skips every
                // later phase; the migration row already says why.
                ProjectPhase::Skipped(detail)
                    if detail != "not cloud-linked" && !receipt.migration.failed() =>
                {
                    Verdict::Info
                }
                ProjectPhase::Skipped(_) | ProjectPhase::Ok(_) | ProjectPhase::Planned(_) => {
                    continue;
                }
            };
            let (cause, remedy) = cause_and_remedy(phase.detail(), output);
            push(&mut findings, Finding {
                verdict,
                who: vec![name.clone()],
                phase: PHASES[index],
                cause,
                remedy,
            });
            found = true;
        }
        if !found {
            healthy.push(name.clone());
            if matches!(&receipt.cloud, ProjectPhase::Skipped(detail) if detail == "not cloud-linked") {
                unlinked += 1;
            }
        }
    }
    if !user_level.is_ok() && !matches!(user_level, ProjectPhase::Skipped(_)) {
        let (cause, remedy) = cause_and_remedy(user_level.detail(), "");
        let verdict = if user_level.failed() { Verdict::Error } else { Verdict::Warning };
        push(&mut findings, Finding {
            verdict,
            who: vec!["user-level store".into()],
            phase: "",
            cause,
            remedy,
        });
    }
    if let Some(error) = hub_transport_error {
        let (cause, remedy) = cause_and_remedy(error, "");
        push(&mut findings, Finding {
            verdict: Verdict::Error,
            who: vec!["hub".into()],
            phase: "serve",
            cause,
            remedy,
        });
    }
    if let Some(hub) = hub {
        if hub.action == "failed" {
            push(&mut findings, Finding {
                verdict: Verdict::Error,
                who: vec!["hub".into()],
                phase: "serve",
                cause: hub.failure.clone().unwrap_or_else(|| "recovery failed".into()),
                remedy: Some(hub.remedy.clone().unwrap_or_else(|| "cas hub restart --force".into())),
            });
        } else if let Some(warning) = &hub.transport_warning {
            push(&mut findings, Finding {
                verdict: Verdict::Warning,
                who: vec!["hub".into()],
                phase: "serve",
                cause: warning.clone(),
                remedy: hub.remedy.clone(),
            });
        } else if hub.prior_state == "none" && hub.remedy.is_some() {
            push(&mut findings, Finding {
                verdict: Verdict::Info,
                who: vec!["hub".into()],
                phase: "",
                cause: "not running".into(),
                remedy: Some("cas hub start".into()),
            });
        }
    }
    for skip in skipped {
        push(&mut findings, Finding {
            verdict: Verdict::Info,
            who: vec![home_relative(&skip.project)],
            phase: "",
            cause: format!("not refreshed: {}", skip.reason),
            remedy: Some("cas known-repos forget <path>".into()),
        });
    }
    findings.sort_by_key(|finding| match finding.verdict {
        Verdict::Error => 0,
        Verdict::Warning => 1,
        Verdict::Info => 2,
        Verdict::Ok => 3,
    });
    SweepFindings {
        findings,
        healthy,
        unlinked,
        user_level_current: user_level.is_ok(),
        project_count: receipts.len(),
        config,
    }
}

/// Collapse a repeat of an existing finding into it.
fn push(findings: &mut Vec<Finding>, finding: Finding) {
    if let Some(same) = findings.iter_mut().find(|existing| {
        existing.verdict == finding.verdict
            && existing.phase == finding.phase
            && existing.cause == finding.cause
            && existing.remedy == finding.remedy
    }) {
        same.who.extend(finding.who);
    } else {
        findings.push(finding);
    }
}

/// A phase detail in reading order: its cause, then the one remedy it names.
pub(super) fn cause_and_remedy(detail: &str, output: &str) -> (String, Option<String>) {
    if let Some(found) = cloud_cause(output) {
        return found;
    }
    let mut detail = detail.trim();
    while let Some(rest) = PHASE_PREFIXES
        .iter()
        .chain(&["cas update: "])
        .find_map(|prefix| detail.strip_prefix(prefix))
    {
        detail = rest.trim_start();
    }
    if let Some((cause, tail)) = detail.rsplit_once("; ")
        && let Some(remedy) = remedy_text(tail)
    {
        return (cause.trim().to_owned(), Some(remedy));
    }
    (detail.to_owned(), None)
}

/// A cloud phase's own summary line says what went wrong and what to run;
/// its detail ("3 pushed; 2 queued") only counts rows.
fn cloud_cause(output: &str) -> Option<(String, Option<String>)> {
    let separators = [
        format!(" {} ", Icons::separator_dot(true)),
        format!(" {} ", Icons::separator_dot(false)),
    ];
    let line = output.lines().find_map(|line| {
        let line = line
            .trim()
            .trim_start_matches("[WARN]")
            .trim_start_matches(Icons::WARNING)
            .trim_start();
        [("Push incomplete", "push incomplete"), ("Pull incomplete", "pull incomplete")]
            .into_iter()
            .find_map(|(prefix, what)| {
                let rest = line.strip_prefix(prefix)?;
                separators.iter().find_map(|separator| {
                    rest.strip_prefix(separator.as_str()).map(|rest| (what, rest, separator))
                })
            })
    })?;
    let (what, rest, separator) = line;
    let mut generic = None;
    let mut specific = None;
    let mut parts: Vec<String> = Vec::new();
    for part in rest.split(separator.as_str()) {
        if let Some(command) = part.strip_prefix("run ") {
            generic = Some(command.trim().to_owned());
            continue;
        }
        if part == "queued rows remain" {
            continue;
        }
        let explains_named_reason = part
            .split_once(": ")
            .is_some_and(|(reason, _)| parts.iter().any(|seen| seen.contains(reason)));
        if !explains_named_reason {
            parts.push(part.to_owned());
            continue;
        }
        // An explanation that only restates a reason the count already
        // named stays under --verbose. One that says what to do instead
        // (a link or a command, cas-7c6b) is the finding's real remedy, and
        // the generic retry it overrides must not be offered.
        if let Some((cause, next)) = guidance(part) {
            parts.extend(cause);
            specific = Some(next);
        }
    }
    Some((format!("{what}: {}", parts.join("; ")), specific.or(generic)))
}

/// Split an explanation of a named reason into what it adds to the cause
/// and what to do, when it says what to do (a link or a backtick command).
fn guidance(part: &str) -> Option<(Option<String>, String)> {
    let actionable = |text: &str| text.contains('`') || text.contains("://");
    let (_, explanation) = part.split_once(": ")?;
    if !actionable(explanation) {
        return None;
    }
    Some(match explanation.rsplit_once(". ") {
        Some((cause, next)) if actionable(next) && !actionable(cause) => (
            Some(part[..part.len() - explanation.len()].to_owned() + cause.trim()),
            next.trim().trim_end_matches('.').to_owned(),
        ),
        _ => (None, explanation.trim().trim_end_matches('.').to_owned()),
    })
}

/// The remedy a detail ends with, as the command itself when it is one.
fn remedy_text(tail: &str) -> Option<String> {
    let tail = tail.trim().trim_end_matches('.');
    if let Some(command) = tail.strip_prefix("run ") {
        let command = command.trim();
        let bare = command.trim_matches('`');
        return Some(if !bare.contains('`') { bare.to_owned() } else { command.to_owned() });
    }
    const VERBS: [&str; 9] = [
        "recheck", "wait", "retry", "rerun", "check", "resolve", "start", "commit", "remove",
    ];
    let first = tail.split_whitespace().next()?.to_ascii_lowercase();
    VERBS.contains(&first.as_str()).then(|| tail.to_owned())
}

/// The shortest trailing path that tells each project apart: `cassy`, and
/// `Richards LLC/Accounting` beside `Documents/Accounting`.
pub(super) fn display_names<'a>(paths: impl Iterator<Item = &'a Path>) -> Vec<String> {
    let parts = paths
        .map(|path| {
            path.components()
                .filter_map(|component| component.as_os_str().to_str())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let suffix = |path: &[String], depth: usize| path[path.len().saturating_sub(depth)..].join("/");
    parts
        .iter()
        .map(|path| {
            (1..=path.len().max(1))
                .map(|depth| suffix(path, depth))
                .find(|name| {
                    parts
                        .iter()
                        .filter(|other| {
                            (1..=other.len()).any(|depth| suffix(other, depth) == *name)
                                && suffix(other, name.matches('/').count() + 1) == *name
                        })
                        .count()
                        == 1
                })
                .unwrap_or_else(|| PathBuf::from_iter(path).display().to_string())
        })
        .collect()
}

/// Print the findings. Returns whether anything was shortened, so the
/// caller names the `--verbose` escape exactly when it is needed.
pub(super) fn print(fmt: &mut Formatter<'_>, sweep: &SweepFindings) -> io::Result<bool> {
    let width = usize::from(fmt.width()).clamp(40, MAX_CELLS);
    let unicode = fmt.unicode();
    let glyphs = fmt.glyphs();
    // One mark column for every row, sized for the widest mark this mode
    // can draw, so `[OK]` and `[ERROR]` rows still align when piped.
    let mark_cells = mark_cells(glyphs);
    let who_cells = sweep
        .findings
        .iter()
        .map(|finding| who_label(finding).chars().count())
        .filter(|cells| *cells <= WHO_CELLS)
        .max()
        .unwrap_or(0)
        .max("user-level".len());
    let lead = 2 + mark_cells + 1;
    let cause_at = lead + who_cells + 1 + PHASE_CELLS + 1;
    let mut shortened = false;
    for finding in &sweep.findings {
        write_mark(fmt, finding.verdict, mark_cells)?;
        let who = who_label(finding);
        if who.chars().count() > who_cells {
            // A name is never cut to fit the column: it gets its own line,
            // and a path too long even for that keeps its end.
            let (who, cut) = fit_path(&who, width.saturating_sub(lead), unicode);
            shortened |= cut;
            fmt.write_bold_plain(&who)?;
            fmt.newline()?;
            fmt.write_raw(&" ".repeat(lead + who_cells + 1))?;
        } else {
            fmt.write_bold_plain(&format!("{who:<who_cells$}"))?;
            fmt.write_raw(" ")?;
        }
        fmt.write_muted(&format!("{:<PHASE_CELLS$}", finding.phase))?;
        fmt.write_raw(" ")?;
        let (lines, cut, continuation_at) = wrap_finding_cause(
            &finding.cause,
            width,
            cause_at,
            lead + 2,
            unicode,
        );
        shortened |= cut;
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                fmt.write_raw(&" ".repeat(continuation_at))?;
            }
            fmt.write_text(line)?;
            fmt.newline()?;
        }
        if finding.who.len() > 1 {
            let (names, cut) = fit(&finding.who.join(", "), width.saturating_sub(cause_at), unicode);
            shortened |= cut;
            fmt.write_raw(&" ".repeat(cause_at))?;
            fmt.write_muted(&names)?;
            fmt.newline()?;
        }
        if let Some(remedy) = &finding.remedy {
            let arrow = Icons::arrow_right(unicode);
            let at = lead + arrow.chars().count() + 1;
            let (lines, cut) = wrap(remedy, width.saturating_sub(at), unicode);
            shortened |= cut;
            for (index, line) in lines.iter().enumerate() {
                if index == 0 {
                    fmt.write_raw(&" ".repeat(lead))?;
                    fmt.write_muted(arrow)?;
                    fmt.write_raw(" ")?;
                } else {
                    fmt.write_raw(&" ".repeat(at))?;
                }
                fmt.write_text(line)?;
                fmt.newline()?;
            }
        }
    }
    if !sweep.healthy.is_empty() {
        write_mark(fmt, Verdict::Ok, mark_cells)?;
        let count = sweep.healthy.len();
        let mut head = if count == sweep.project_count {
            format!("all {count} projects current")
        } else {
            format!("{count} {} current", if count == 1 { "project" } else { "projects" })
        };
        if sweep.unlinked > 0 {
            head.push_str(&format!(" · {} not cloud-linked", sweep.unlinked));
        }
        let (head, cut) = fit(&head, width.saturating_sub(lead), unicode);
        shortened |= cut;
        fmt.write_text(&head)?;
        fmt.newline()?;
        let (names, cut) = fit(&sweep.healthy.join(", "), width.saturating_sub(lead), unicode);
        shortened |= cut;
        fmt.write_raw(&" ".repeat(lead))?;
        fmt.write_muted(&names)?;
        fmt.newline()?;
    }
    if sweep.config.repaired > 0 {
        write_mark(fmt, Verdict::Ok, mark_cells)?;
        let text = format!(
            "config  {} retired {} removed in {} {}",
            sweep.config.entries_removed,
            if sweep.config.entries_removed == 1 {
                "entry"
            } else {
                "entries"
            },
            sweep.config.repaired,
            if sweep.config.repaired == 1 {
                "file"
            } else {
                "files"
            }
        );
        let (lines, cut) = wrap(&text, width.saturating_sub(lead), unicode);
        shortened |= cut;
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                fmt.write_raw(&" ".repeat(lead))?;
            }
            fmt.write_text(line)?;
            fmt.newline()?;
        }
    }
    if sweep.user_level_current {
        write_mark(fmt, Verdict::Ok, mark_cells)?;
        fmt.write_text("user-level store current")?;
        fmt.newline()?;
    }
    Ok(shortened)
}

/// The widest mark the current mode draws: a glyph, or `[ERROR]`.
pub(super) fn mark_cells(glyphs: bool) -> usize {
    [Verdict::Ok, Verdict::Warning, Verdict::Error, Verdict::Info]
        .into_iter()
        .map(|verdict| verdict.label(glyphs).chars().count())
        .max()
        .unwrap_or(1)
}

fn write_mark(fmt: &mut Formatter<'_>, verdict: Verdict, cells: usize) -> io::Result<()> {
    fmt.write_raw("  ")?;
    fmt.mark(verdict)?;
    let drawn = verdict.label(fmt.glyphs()).chars().count();
    fmt.write_raw(&" ".repeat(cells.saturating_sub(drawn) + 1))
}

fn who_label(finding: &Finding) -> String {
    match finding.who.as_slice() {
        [one] => one.clone(),
        many => format!("{} projects", many.len()),
    }
}

/// A path shortened from its start, so the project name survives.
fn fit_path(text: &str, cells: usize, unicode: bool) -> (String, bool) {
    if text.chars().count() <= cells {
        return (text.to_owned(), false);
    }
    let ellipsis = if unicode { "…" } else { "..." };
    let keep = cells.saturating_sub(ellipsis.chars().count());
    let skip = text.chars().count() - keep;
    (format!("{ellipsis}{}", text.chars().skip(skip).collect::<String>()), true)
}

/// A path under the home directory, written from `~`.
pub(crate) fn home_relative(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

fn fit(text: &str, cells: usize, unicode: bool) -> (String, bool) {
    if text.chars().count() <= cells {
        return (text.to_owned(), false);
    }
    let ellipsis = if unicode { "…" } else { "..." };
    let keep = cells.saturating_sub(ellipsis.chars().count());
    (format!("{}{ellipsis}", text.chars().take(keep).collect::<String>()), true)
}

/// Words of prose, with a backtick-quoted command or a short parenthetical
/// (`(age 15d)`) kept as one word so a wrap never splits it.
fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut quoted = false;
    let mut depth = 0usize;
    for word in text.split_whitespace() {
        match words.last_mut() {
            Some(last) if quoted || (depth > 0 && last.chars().count() < 24) => {
                last.push(' ');
                last.push_str(word);
            }
            _ => words.push(word.to_owned()),
        }
        if word.matches('`').count() % 2 == 1 {
            quoted = !quoted;
        }
        depth = (depth + word.matches('(').count()).saturating_sub(word.matches(')').count());
    }
    words
}

fn wrap_finding_cause(
    cause: &str,
    width: usize,
    cause_at: usize,
    continuation_at: usize,
    unicode: bool,
) -> (Vec<String>, bool, usize) {
    const INACTIVE_SERVE: &str = "Tailscale Serve inactive: ";
    let Some(reason) = cause.strip_prefix(INACTIVE_SERVE) else {
        let (lines, shortened) = wrap(cause, width.saturating_sub(cause_at), unicode);
        return (lines, shortened, cause_at);
    };
    let (mut lines, shortened) = wrap(reason, width.saturating_sub(continuation_at), unicode);
    lines.insert(0, "Tailscale Serve inactive:".to_owned());
    (lines, shortened, continuation_at)
}

/// Wrap prose at word boundaries; a single word wider than the column is
/// shortened rather than split.
pub(super) fn wrap(text: &str, cells: usize, unicode: bool) -> (Vec<String>, bool) {
    let cells = cells.max(16);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut shortened = false;
    for word in words(text) {
        let (word, cut) = fit(&word, cells, unicode);
        shortened |= cut;
        let needed = if line.is_empty() { 0 } else { 1 } + word.chars().count();
        if !line.is_empty() && line.chars().count() + needed > cells {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    (lines, shortened)
}
