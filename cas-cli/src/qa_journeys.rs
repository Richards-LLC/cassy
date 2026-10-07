//! Affected-journey receipts at worker close/QA; full receipts at epic assembly.
use super::{EvidenceContext, EvidenceRefusal, cited_bundle_path, git, is_full_sha};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const SELECTION_PREFIX: &str = "affected-journeys:";
pub const RECEIPT_CITATION: &str = "journey-receipt:";

#[cfg(test)]
#[path = "qa_journeys_tests.rs"]
mod tests;

#[derive(Debug, Deserialize)]
struct JourneyResult {
    id: String,
    status: String,
    passed: u64,
    failed: u64,
    skipped: u64,
}

#[derive(Debug, Deserialize)]
struct JourneyReceipt {
    schema: u32,
    producer: String,
    kind: String,
    scope: String,
    base_sha: String,
    head_sha: String,
    selection_ids: Vec<String>,
    results: Vec<JourneyResult>,
    tool_version: String,
    suite_exit: i64,
    #[serde(default)]
    ci_run_url: String,
    #[serde(default)]
    executed_head_sha: String,
}

pub fn affects_hub(paths: &[String]) -> bool {
    paths
        .iter()
        .any(|p| p.starts_with("hub-web/src/") || p.starts_with("hub-web/dist/"))
}

/// Fail closed: a broken selector is not an empty impact selection. The
/// reviewed revision is explicit even when the caller checked out another tip.
pub fn select_journeys(
    repo: &Path,
    base: &str,
    head: &str,
    paths: Option<&[String]>,
) -> Result<Vec<String>, String> {
    if !is_full_sha(base)
        || !is_full_sha(head)
        || git(repo, &["merge-base", "--is-ancestor", base, head]).is_none()
    {
        return Err(
            "journey selection requires a readable ancestor base and exact full head SHA".into(),
        );
    }
    // The store checkout may predate the delivery's selector and catalog.
    // Pin the executable as well as its data to the reviewed revision; an
    // older executable may not understand the HEAD/BASE environment at all.
    let source = git(repo, &["show", &format!("{head}:scripts/journeys-for-diff.py")])
        .ok_or_else(|| format!("journeys-for-diff selection failed at {head}: committed selector is unreadable"))?;
    let mut command = Command::new("python3");
    command.arg("-");
    if let Some(paths) = paths {
        command.arg("--paths").args(paths);
    } else {
        command.arg("--all");
    }
    let mut child = command
        .current_dir(repo)
        .env("CAS_JOURNEYS_ROOT", repo)
        .env("CAS_JOURNEYS_BASE", base)
        .env("CAS_JOURNEYS_HEAD", head)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("journey selection failed to start: {e}"))?;
    let written = child.stdin.take().ok_or_else(|| "selector input is unavailable".to_string())
        .and_then(|mut input| input.write_all(source.as_bytes()).map_err(|e| e.to_string()));
    if let Err(error) = written {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("journey selection failed to load committed selector: {error}"));
    }
    let output = child.wait_with_output()
        .map_err(|e| format!("journey selection failed to finish: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "journeys-for-diff selection failed at {head}; repair the selector before recording proof"
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("journey selection is not JSON: {e}"))?;
    let rows = value
        .get("journeys")
        .and_then(|v| v.as_array())
        .ok_or("journey selection has no journeys array")?;
    let mut ids = BTreeSet::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("journey selection contains an unnamed journey")?;
        if !ids.insert(id.to_string()) {
            return Err(format!("journey selection repeats {id}"));
        }
    }
    Ok(ids.into_iter().collect())
}

pub fn selection_reason(base: &str, ids: &[String]) -> String {
    format!("{SELECTION_PREFIX} base={base} ids={}", ids.join(","))
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix(key))
}

/// Saved by the successful implementer gate. QA uses this immutable tip's
/// selection, rather than a merge-base which collapses after integration.
pub fn recorded_selection(notes: &str, head: &str) -> Option<(String, Vec<String>)> {
    notes
        .lines()
        .filter_map(|line| {
            let line = line.split_once("JOURNEY_SELECTION:")?.1;
            if field(line, "head=")? != head {
                return None;
            }
            let base = field(line, "base=")?.to_string();
            let ids = field(line, "ids=")
                .unwrap_or("")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            Some((base, ids))
        })
        .last()
}

pub fn doc_only_rebind(repo: &Path, executed: &str, head: &str) -> bool {
    if !is_full_sha(executed) || !is_full_sha(head) {
        return false;
    }
    if executed == head {
        return git(repo, &["rev-parse", "--verify", head]).is_some();
    }
    if git(repo, &["merge-base", "--is-ancestor", executed, head]).is_none() {
        return false;
    }
    let Some(changed) = git(repo, &["diff", "--name-only", executed, head]) else {
        return false;
    };
    if changed.is_empty() || !changed.lines().all(crate::qa_pass::is_non_surface_path) {
        return false;
    }
    git(
        repo,
        &[
            "diff",
            "--quiet",
            executed,
            head,
            "--",
            "hub-web",
            "scripts/journeys-for-diff.py",
            "scripts/journey-eval.sh",
            "scripts/journey-bundles.py",
            "scripts/journey-receipt.py",
            "docs/qa/journeys.md",
            "package.json",
            "package-lock.json",
        ],
    )
    .is_some()
}

fn repair_command(ctx: &EvidenceContext<'_>, base: &str, full: bool) -> String {
    if full {
        format!(
            "scripts/journey-eval.sh {} --full --workers=4 (no journey filter); cite {RECEIPT_CITATION} <absolute receipt path> on the epic",
            ctx.task_artifacts_dir.display()
        )
    } else {
        format!(
            "scripts/journey-eval.sh {} --affected {base} --workers=4 at {}; default journey-eval uses affected selection; selection can be inspected with scripts/journeys-for-diff.py {base} {}",
            ctx.task_artifacts_dir.display(),
            ctx.delivered_head,
            ctx.delivered_head
        )
    }
}

/// All referenced evidence must stay in the owning task's artifact namespace.
fn owned_path(ctx: &EvidenceContext<'_>, directory: &Path, cited: &str) -> Result<PathBuf, String> {
    let root = ctx
        .task_artifacts_dir
        .canonicalize()
        .map_err(|e| format!("task artifacts unreadable: {e}"))?;
    let path = directory
        .join(super::expand_home(cited))
        .canonicalize()
        .map_err(|e| format!("journey receipt unreadable: {e}"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Err("journey receipt escapes the owning task artifact directory".into());
    }
    Ok(path)
}

pub fn validate_journey_receipt(
    ctx: &EvidenceContext<'_>,
    path: &Path,
    base: &str,
    ids: &[String],
    full: bool,
) -> Result<PathBuf, EvidenceRefusal> {
    let fail = |problem: String| EvidenceRefusal::new(problem, repair_command(ctx, base, full));
    let path = owned_path(ctx, ctx.task_artifacts_dir, &path.to_string_lossy()).map_err(&fail)?;
    let receipt: JourneyReceipt = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| fail(format!("unreadable journey receipt: {e}")))?,
    )
    .map_err(|e| fail(format!("malformed journey receipt: {e}")))?;
    if receipt.schema != 1
        || receipt.producer != "journey-eval"
        || !matches!(receipt.kind.as_str(), "local" | "ci")
        || !matches!(receipt.scope.as_str(), "affected" | "full")
        || receipt.tool_version.trim().is_empty()
        || receipt.suite_exit != 0
    {
        return Err(fail(
            "journey receipt lacks successful native runner provenance".into(),
        ));
    }
    if receipt.head_sha != ctx.delivered_head
        || !is_full_sha(&receipt.head_sha)
        || !is_full_sha(&receipt.base_sha)
        || git(
            ctx.repo,
            &[
                "merge-base",
                "--is-ancestor",
                &receipt.base_sha,
                &receipt.head_sha,
            ],
        )
        .is_none()
        || (receipt.scope == "affected" && receipt.base_sha != base)
    {
        return Err(fail(
            "journey receipt does not bind the exact delivered tip and selection base".into(),
        ));
    }
    if !receipt.executed_head_sha.is_empty()
        && !doc_only_rebind(ctx.repo, &receipt.executed_head_sha, &receipt.head_sha)
    {
        return Err(fail(
            "journey receipt rebind changed the evaluated product or journey inputs".into(),
        ));
    }
    if full && receipt.scope != "full" {
        return Err(fail(
            "epic assembly requires a full-suite journey receipt".into(),
        ));
    }
    if receipt.kind == "ci"
        && !(receipt.ci_run_url.starts_with("https://github.com/")
            && receipt.ci_run_url.contains("/actions/runs/"))
    {
        return Err(fail(
            "CI journey receipt has no GitHub Actions run URL".into(),
        ));
    }
    let selected: BTreeSet<_> = receipt.selection_ids.iter().cloned().collect();
    let results: BTreeSet<_> = receipt.results.iter().map(|r| r.id.clone()).collect();
    if selected.len() != receipt.selection_ids.len() || results.len() != receipt.results.len() {
        return Err(fail(
            "journey receipt repeats selection or result IDs".into(),
        ));
    }
    let missing: Vec<_> = ids
        .iter()
        .filter(|id| !selected.contains(*id) || !results.contains(*id))
        .cloned()
        .collect();
    let failed: Vec<_> = receipt
        .results
        .iter()
        .filter(|r| (r.status != "PASS" || r.passed == 0 || r.failed != 0 || r.skipped != 0))
        .map(|r| r.id.clone())
        .collect();
    let unrecorded: Vec<_> = selected
        .difference(&results)
        .map(|s| s.to_string())
        .collect();
    if !missing.is_empty() || !failed.is_empty() || !unrecorded.is_empty() {
        return Err(fail(format!(
            "journey receipt missing selected IDs [{}]; nonpassing IDs [{}]; unrecorded IDs [{}]",
            missing.join(", "),
            failed.join(", "),
            unrecorded.join(", ")
        )));
    }
    if full && ids.is_empty() {
        return Err(fail("full-suite catalog selection is empty".into()));
    }
    Ok(path)
}

pub fn validate_bundle_journeys(
    ctx: &EvidenceContext<'_>,
    manifest: &Path,
    base: &str,
    ids: &[String],
) -> Result<PathBuf, EvidenceRefusal> {
    let fail = |problem: String| EvidenceRefusal::new(problem, repair_command(ctx, base, false));
    let manifest =
        owned_path(ctx, ctx.task_artifacts_dir, &manifest.to_string_lossy()).map_err(&fail)?;
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest).map_err(|e| fail(format!("bundle unreadable: {e}")))?,
    )
    .map_err(|e| fail(format!("bundle is not JSON: {e}")))?;
    let citation = value
        .get("journey_receipt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            fail(format!(
                "missing journey_receipt for selected IDs [{}]",
                ids.join(", ")
            ))
        })?;
    let path = owned_path(
        ctx,
        manifest.parent().unwrap_or(ctx.task_artifacts_dir),
        citation,
    )
    .map_err(&fail)?;
    validate_journey_receipt(ctx, &path, base, ids, false)
}

pub fn check_close_journeys(
    ctx: &EvidenceContext<'_>,
    reasons: &[String],
) -> Result<Option<String>, EvidenceRefusal> {
    let Some(reason) = reasons
        .iter()
        .find(|r| r.starts_with(SELECTION_PREFIX) || r.as_str() == "affected-journeys")
    else {
        return Ok(None);
    };
    let base = field(reason, "base=").unwrap_or(ctx.delivered_head);
    let ids: Vec<String> = if let Some(ids) = field(reason, "ids=") {
        ids.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        reasons
            .iter()
            .filter_map(|r| r.strip_prefix("journeys:"))
            .flat_map(|s| s.split(','))
            .map(str::to_string)
            .collect()
    };
    let citation = cited_bundle_path(ctx.notes).ok_or_else(|| {
        EvidenceRefusal::new(
            format!(
                "missing journey receipt for selected IDs [{}]",
                ids.join(", ")
            ),
            repair_command(ctx, base, false),
        )
    })?;
    validate_bundle_journeys(ctx, Path::new(&citation), base, &ids)?;
    Ok(Some(format!(
        "JOURNEY_SELECTION: head={} base={base} ids={}",
        ctx.delivered_head,
        if ids.is_empty() {
            " (no affected journeys)".into()
        } else {
            ids.join(",")
        }
    )))
}

pub fn check_epic_journeys(ctx: &EvidenceContext<'_>) -> Result<PathBuf, EvidenceRefusal> {
    let ids = select_journeys(ctx.repo, ctx.delivered_head, ctx.delivered_head, None)
        .map_err(|e| EvidenceRefusal::new(e, repair_command(ctx, ctx.delivered_head, true)))?;
    let citation = ctx
        .notes
        .lines()
        .filter_map(|line| {
            line.split_once(RECEIPT_CITATION)
                .and_then(|(_, rest)| rest.split_whitespace().next())
        })
        .last()
        .ok_or_else(|| {
            EvidenceRefusal::new(
                "epic assembly has no full-suite journey receipt",
                repair_command(ctx, ctx.delivered_head, true),
            )
        })?;
    validate_journey_receipt(ctx, Path::new(citation), ctx.delivered_head, &ids, true)
}

/// A failed QA verdict may preserve failing journeys. Approval must cover the
/// exact selection recorded by the implementer's successful close gate.
pub fn check_round_journeys(
    ctx: &EvidenceContext<'_>,
    manifest: &Path,
    delivery_notes: &str,
    paths: &[String],
) -> Result<Option<PathBuf>, EvidenceRefusal> {
    let Some((base, mut ids)) = recorded_selection(delivery_notes, ctx.delivered_head) else {
        if affects_hub(paths) {
            return Err(EvidenceRefusal::new(
                "delivery has no affected-journey selection for this QA tip",
                "re-close the delivery to record its exact-tip selection, then run scripts/journey-eval.sh for every selected ID",
            ));
        }
        return Ok(None);
    };
    if !paths.is_empty() {
        ids = select_journeys(ctx.repo, &base, ctx.delivered_head, Some(paths))
            .map_err(|e| EvidenceRefusal::new(e, repair_command(ctx, &base, false)))?;
    }
    validate_bundle_journeys(ctx, manifest, &base, &ids).map(Some)
}
