//! Journey selection helpers that QA reasons and evidence rebinds read
//! (cas-8132). The affected- and full-suite journey receipt gates at close,
//! QA round and epic assembly were removed in cas-38e7: the merge queue runs
//! the journeys once.
use super::{git, is_full_sha};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg(test)]
#[path = "qa_journeys_tests.rs"]
mod tests;

/// cas-8132: the catalog journeys `paths` touch, judged by the selector and
/// catalog committed at `head` (no base: CSS/fixture hunk context is not
/// needed to name the touched journeys). Used for a delivery's "journeys:"
/// reason, so it never reads the store checkout's working tree.
pub fn journeys_for_paths_at(
    repo: &Path,
    head: &str,
    paths: &[String],
) -> Result<Vec<String>, String> {
    journeys_for_paths_at_with_base(repo, head, paths, None)
}

/// Registry selection retains CSS/composition/fixture context from the delivery base.
pub(crate) fn journeys_for_paths_at_with_base(
    repo: &Path,
    head: &str,
    paths: &[String],
    base: Option<&str>,
) -> Result<Vec<String>, String> {
    let head = git(
        repo,
        &["rev-parse", "--verify", &format!("{head}^{{commit}}")],
    )
    .map(|sha| sha.trim().to_string())
    .filter(|sha| is_full_sha(sha))
    .ok_or_else(|| format!("journey selection head {head} is unreadable"))?;
    if git(
        repo,
        &["cat-file", "-e", &format!("{head}:docs/qa/journeys.md")],
    )
    .is_none()
    {
        return Ok(Vec::new());
    }
    run_committed_selector(repo, &head, paths, base)
}

fn run_committed_selector(
    repo: &Path,
    head: &str,
    paths: &[String],
    base: Option<&str>,
) -> Result<Vec<String>, String> {
    // The store checkout may predate the delivery's selector and catalog.
    // Pin the executable as well as its data to the reviewed revision; an
    // older executable may not understand the HEAD/BASE environment at all.
    let source = git(
        repo,
        &["show", &format!("{head}:scripts/journeys-for-diff.py")],
    )
    .ok_or_else(|| {
        format!("journeys-for-diff selection failed at {head}: committed selector is unreadable")
    })?;
    let mut command = Command::new("python3");
    command.arg("-");
    command.arg("--paths").args(paths);
    command
        .current_dir(repo)
        .env("CAS_JOURNEYS_ROOT", repo)
        .env("CAS_JOURNEYS_HEAD", head)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env_remove("CAS_JOURNEYS_BASE");
    if let Some(base) = base {
        command.env("CAS_JOURNEYS_BASE", base);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("journey selection failed to start: {e}"))?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| "selector input is unavailable".to_string())
        .and_then(|mut input| {
            input
                .write_all(source.as_bytes())
                .map_err(|e| e.to_string())
        });
    if let Err(error) = written {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "journey selection failed to load committed selector: {error}"
        ));
    }
    let output = child
        .wait_with_output()
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

fn is_hub_web_documentation_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let test_input = lower.split('/').any(|segment| {
        matches!(
            segment,
            "e2e"
                | "test"
                | "tests"
                | "__tests__"
                | "testdata"
                | "fixture"
                | "__fixtures__"
                | "__mocks__"
        ) || segment.ends_with("-fixtures")
            || segment.ends_with("_fixtures")
    });
    !test_input
        && [".md", ".mdx", ".rst", ".adoc"]
            .iter()
            .any(|ext| file.ends_with(ext))
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
    let changed_paths: Vec<_> = changed.lines().collect();
    if changed_paths.is_empty()
        || !changed_paths
            .iter()
            .all(|path| crate::qa_pass::is_non_surface_path(path))
        || changed_paths
            .iter()
            .any(|path| path.starts_with("hub-web/") && !is_hub_web_documentation_path(path))
    {
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
