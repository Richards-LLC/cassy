//! Epic-close CI authority: required checks, or the exact merge's proven tree.

use super::{BRANCH_CI_LOOKUP_TIMEOUT, GhApiOutput, ci_git, fetch_branch_ci, gh_api_get};
use serde_json::Value;
use std::path::Path;

#[derive(serde::Deserialize)]
struct RequiredCheck {
    context: String,
    app_id: Option<i64>,
}

fn required_checks(output: GhApiOutput) -> Result<Vec<RequiredCheck>, String> {
    let unavailable = "required-check metadata unavailable (API failure, unprotected branch or no declared required checks); cannot establish CI authority";
    if !output.success {
        return Err(format!("{unavailable}: {}", output.status));
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| unavailable)?;
    let mut checks: Vec<RequiredCheck> = serde_json::from_value(
        value
            .get("checks")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    )
    .map_err(|_| unavailable)?;
    // Legacy protection metadata binds contexts without an app restriction.
    if checks.is_empty() {
        let contexts: Vec<String> = serde_json::from_value(
            value
                .get("contexts")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| unavailable)?;
        checks = contexts
            .into_iter()
            .map(|context| RequiredCheck {
                context,
                app_id: None,
            })
            .collect();
    }
    if checks.is_empty() || checks.iter().any(|check| check.context.trim().is_empty()) {
        return Err(unavailable.into());
    }
    Ok(checks)
}

fn completed_checks(
    output: GhApiOutput,
    sha: &str,
    allow_skipped: bool,
) -> Result<Vec<Value>, String> {
    let invalid =
        || format!("CI check-runs on exact SHA {sha} are missing, incomplete or unsuccessful");
    if !output.success {
        return Err(invalid());
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|_| invalid())?;
    let runs = value
        .get("check_runs")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if runs.is_empty()
        || value
            .get("total_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > runs.len() as u64)
        || runs.iter().any(|run| {
            run.get("status").and_then(Value::as_str) != Some("completed")
                || run
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|name| name.trim().is_empty())
                || run
                    .get("head_sha")
                    .and_then(Value::as_str)
                    .is_some_and(|head| head != sha)
                || !(conclusion(run) == Some("success")
                    || (allow_skipped && conclusion(run) == Some("skipped")))
        })
    {
        return Err(invalid());
    }
    Ok(runs.clone())
}

fn conclusion(run: &Value) -> Option<&str> {
    run.get("conclusion").and_then(Value::as_str)
}

fn matches_required(run: &Value, required: &RequiredCheck) -> bool {
    run.get("name").and_then(Value::as_str) == Some(required.context.as_str())
        && required
            .app_id
            .filter(|id| *id > 0)
            .is_none_or(|id| run.pointer("/app/id").and_then(Value::as_i64) == Some(id))
}

fn require_coverage(runs: &[Value], required: &[RequiredCheck]) -> Result<(), String> {
    for check in required {
        if !runs.iter().any(|run| matches_required(run, check)) {
            return Err(format!(
                "necessary CI check {:?} (app {:?}) is missing or unsuccessful",
                check.context, check.app_id
            ));
        }
    }
    Ok(())
}

pub(crate) fn epic_tip_ci_proof(sha: &str, default: &str, cwd: &Path) -> Result<String, String> {
    let binary = crate::github_repo::gh_binary();
    let repo = crate::github_repo::resolve_origin(cwd, &binary, BRANCH_CI_LOOKUP_TIMEOUT)?;
    let required = required_checks(gh_api_get(
        &format!(
            "repos/{}/branches/{}/protection/required_status_checks",
            repo.canonical,
            urlencoding::encode(default)
        ),
        cwd,
        &binary,
    ))?;
    let authority = required
        .iter()
        .map(|check| format!("{}@{:?}", check.context, check.app_id))
        .collect::<Vec<_>>()
        .join(", ");
    let target = completed_checks(
        fetch_branch_ci(sha, cwd, &repo.canonical, &binary),
        sha,
        true,
    )?;
    require_coverage(&target, &required)?;
    if target.iter().all(|run| conclusion(run) == Some("success")) {
        return Ok(format!(
            "CI check-runs on exact SHA {sha} passed required checks: {authority}"
        ));
    }
    if !target.iter().any(|run| conclusion(run) == Some("success")) {
        return Err(format!("CI on {sha} has no successful post-merge check"));
    }

    // No ancestor search: only this merge's second parent may supply proof.
    // Tree equality includes the workflow definitions and all test/config bytes.
    let deadline = crate::bounded_process::Deadline::after(BRANCH_CI_LOOKUP_TIMEOUT);
    let merge = ci_git(
        cwd,
        &["show", "--no-patch", "--format=%P%n%T", sha],
        deadline,
    )?;
    let merge = String::from_utf8_lossy(&merge);
    let mut lines = merge.lines();
    let parents = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    let tree = lines
        .next()
        .filter(|tree| !tree.is_empty())
        .ok_or("merge tree is unavailable")?;
    if parents.len() != 2 {
        return Err(format!(
            "CI skips on {sha} require an exact two-parent merge tree receipt"
        ));
    }
    let source = parents[1];
    let source_tree = ci_git(
        cwd,
        &["rev-parse", "--verify", &format!("{source}^{{tree}}")],
        deadline,
    )?;
    if String::from_utf8_lossy(&source_tree).trim() != tree {
        return Err(format!(
            "merge {sha} differs from source {source}; CI cannot cover changed code or workflow/config"
        ));
    }
    let proven = completed_checks(
        fetch_branch_ci(source, cwd, &repo.canonical, &binary),
        source,
        false,
    )?;
    require_coverage(&proven, &required)?;
    for skipped in target
        .iter()
        .filter(|run| conclusion(run) == Some("skipped"))
    {
        let app = skipped
            .pointer("/app/id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0);
        if app.is_none()
            || !proven.iter().any(|run| {
                run.get("name") == skipped.get("name")
                    && run.pointer("/app/id").and_then(Value::as_i64) == app
            })
        {
            return Err(format!(
                "skipped check {:?} has no successful source check with the same name/app identity",
                skipped.get("name")
            ));
        }
    }
    Ok(format!(
        "CI exact-tree receipt: merge={sha} source={source} tree={tree}; required checks passed: {authority}; all skipped jobs covered by successful source checks with the same name/app identity"
    ))
}
