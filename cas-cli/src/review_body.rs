//! Shared review body. Reversibility is descriptive; no policy consumes it.
use anyhow::{Context, Result, bail};
use cas_types::Task;
use std::path::Path;
use std::process::Command;

/// Red-first stub (cas-8b0e).
const CAS_8B0E_RED_STUB: bool = true;

const TEMPLATE: &str = include_str!("../../docs/review/pr-body.md");

pub(crate) struct ReviewBody<'a> {
    pub summary: &'a str,
    pub before: &'a str,
    pub after: &'a str,
    pub evidence: &'a str,
    pub details: &'a str,
    pub risk: &'a str,
    pub door: &'a str,
}

impl ReviewBody<'_> {
    pub fn render(&self) -> String {
        // Split the original template once: user text containing another
        // template token must stay literal rather than be substituted again.
        let mut output = String::new();
        for (index, part) in TEMPLATE.split("{{").enumerate() {
            if index == 0 {
                output.push_str(part);
                continue;
            }
            let (key, rest) = part.split_once("}}").expect("review template token");
            output.push_str(match key {
                "summary" => self.summary,
                "before" => self.before,
                "after" => self.after,
                "evidence" => self.evidence,
                "details" => self.details,
                "risk" => self.risk,
                "door" => self.door,
                _ => panic!("unknown review template token: {key}"),
            });
            output.push_str(rest);
        }
        output
    }
}

pub(crate) fn task_risk(task: &Task) -> String {
    if task.risk.is_empty() {
        "not declared".into()
    } else {
        task.risk
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Links to the evidence recorded on the task (`qa-bundle:` and
/// `base-vs-change:` note lines).
fn task_evidence_references(task: &Task) -> Vec<String> {
    task.notes
        .lines()
        .filter_map(|line| {
            for (marker, label) in [
                ("qa-bundle:", "QA bundle"),
                ("base-vs-change:", "Base-vs-change run"),
            ] {
                if let Some((_, reference)) = line.split_once(marker) {
                    let reference = reference.trim();
                    if !reference.is_empty() {
                        return Some(format!("[{label}](<{reference}>)"));
                    }
                }
            }
            None
        })
        .collect()
}

/// The PR-body Evidence section. With nothing recorded it prompts the author.
pub(crate) fn task_evidence(task: &Task) -> String {
    let references = task_evidence_references(task);
    if references.is_empty() {
        "Evidence not supplied; attach a QA bundle or verify-before-claim base-vs-change run."
            .into()
    } else {
        references.join("\n")
    }
}

/// cas-8b0e: the worker return contract's `proof:` and `ci:` lines.
pub(crate) fn contract_evidence(message: &str) -> Vec<String> {
    if CAS_8B0E_RED_STUB {
        return Vec::new();
    }
    message
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            ["proof", "ci"].into_iter().find_map(|key| {
                let (head, value) = line.split_once(':')?;
                let value = value.trim();
                (head.trim().eq_ignore_ascii_case(key) && !value.is_empty())
                    .then(|| format!("{key}: {value}"))
            })
        })
        .collect()
}

/// cas-8b0e: the review block a supervisor reads inside a merge request.
///
/// It is a summary for someone who acts on it, not a form to fill in: the
/// evidence is the worker's own `proof:`/`ci:` lines plus any QA bundle
/// recorded on the task, and a field nobody supplied renders as `—`. The
/// prompting wording ("not supplied", "not declared") belongs to the PR body
/// generator ([`delivery_body`]) only, where it asks the author for it.
pub(crate) fn merge_request_review(
    repo: &Path,
    task: &Task,
    base: &str,
    head: &str,
    worker_message: &str,
) -> String {
    if CAS_8B0E_RED_STUB {
        return delivery_body(repo, Some(task), base, head, None, None, None).unwrap_or_default();
    }
    let (summary, details) = match delivery_summary(repo, base, head) {
        Ok(summary) => (summary, String::new()),
        Err(error) => (
            format!("```text\n{base} -> {head}\n```"),
            format!("\nGit comparison unavailable: {error}\n"),
        ),
    };
    let mut evidence: Vec<String> = contract_evidence(worker_message)
        .into_iter()
        .chain(task_evidence_references(task))
        .map(|line| format!("- {line}"))
        .collect();
    if evidence.is_empty() {
        evidence.push("—".into());
    }
    let risk = if task.risk.is_empty() {
        "—".to_string()
    } else {
        task_risk(task)
    };
    let door = task
        .door
        .map(|door| door.to_string())
        .unwrap_or_else(|| "—".into());
    format!(
        "## Summary\n\n{summary}\n\n## Evidence\n\n{}\n{details}\n## Merge Danger\n\n\
         **Risk:** {risk}\n\n**Door:** {door}\n",
        evidence.join("\n")
    )
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(repo).args(args).output()?;
    if !output.status.success() {
        bail!(
            "git review comparison failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim_end().to_owned())
}

/// Resolve both refs to immutable commits; no shell interpolation.
fn resolve_pair(repo: &Path, base: &str, head: &str) -> Result<(String, String)> {
    let base_sha = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{base}^{{commit}}"),
        ],
    )
    .context("review base does not resolve")?;
    let head_sha = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{head}^{{commit}}"),
        ],
    )
    .context("review head does not resolve")?;
    Ok((base_sha, head_sha))
}

/// The changed-file tree between `base` and `head`.
fn delivery_summary(repo: &Path, base: &str, head: &str) -> Result<String> {
    let (base_sha, head_sha) = resolve_pair(repo, base, head)?;
    let changes = git(
        repo,
        &["diff", "--name-status", &format!("{base_sha}...{head_sha}")],
    )?;
    // Git quotes control characters in paths. A text tree is the smallest
    // honest visual for an automatically generated delivery summary.
    let summary = format!(
        "```text\n{base_sha}\n  -> {head_sha}\n{}\n```",
        if changes.is_empty() {
            "(no changed files)"
        } else {
            &changes
        }
    );
    Ok(summary)
}

/// Resolve both refs to immutable commits before diffing; no shell interpolation.
pub(crate) fn delivery_body(
    repo: &Path,
    task: Option<&Task>,
    base: &str,
    head: &str,
    before: Option<&str>,
    after: Option<&str>,
    evidence: Option<&str>,
) -> Result<String> {
    let (base_sha, head_sha) = resolve_pair(repo, base, head)?;
    let summary = delivery_summary(repo, &base_sha, &head_sha)?;
    let before_default = format!("Baseline `{base_sha}`; behavior run not supplied.");
    let after_default = format!("Change `{head_sha}`; behavior run not supplied.");
    let evidence_default = task.map(task_evidence).unwrap_or_else(|| {
        "Evidence not supplied; attach a QA bundle or verify-before-claim base-vs-change run."
            .into()
    });
    let risk = task.map(task_risk).unwrap_or_else(|| "not declared".into());
    let door = task
        .and_then(|task| task.door)
        .map(|door| door.to_string())
        .unwrap_or_else(|| "not declared".into());
    Ok(ReviewBody {
        summary: &summary,
        before: before.unwrap_or(&before_default),
        after: after.unwrap_or(&after_default),
        evidence: evidence.unwrap_or(&evidence_default),
        details: "",
        risk: &risk,
        door: &door,
    }
    .render())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_delivery_and_release_fixtures() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../docs/review/pr-body-fixtures.json")).unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let fields = &fixture["fields"];
            let field = |key: &str| fields[key].as_str().unwrap();
            let actual = ReviewBody {
                summary: field("summary"),
                before: field("before"),
                after: field("after"),
                evidence: field("evidence"),
                details: field("details"),
                risk: field("risk"),
                door: field("door"),
            }
            .render();
            assert_eq!(
                actual,
                fixture["expected"].as_str().unwrap(),
                "{}",
                fixture["name"]
            );
        }
    }

    #[test]
    fn real_git_delivery_uses_recorded_metadata_and_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q"]).unwrap();
        git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
        git(repo, &["config", "user.name", "Fixture"]).unwrap();
        std::fs::write(repo.join("input.txt"), "before\n").unwrap();
        git(repo, &["add", "."]).unwrap();
        git(repo, &["commit", "-qm", "base"]).unwrap();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(repo.join("input.txt"), "after\n").unwrap();
        git(repo, &["commit", "-qam", "change"]).unwrap();
        let mut task = Task::new("cas-fixture".into(), "Fixture".into());
        task.risk = vec![cas_types::TaskRisk::BlastRadius];
        task.door = Some(cas_types::TaskDoor::OneWay);
        task.notes = "qa-bundle: /proof/qa/bundle.json".into();
        let body = delivery_body(
            repo,
            Some(&task),
            &base,
            "HEAD",
            Some("fails"),
            Some("passes"),
            None,
        )
        .unwrap();
        assert!(body.contains("M\tinput.txt"));
        assert!(body.contains("**Before:** fails"));
        assert!(body.contains("**After:** passes"));
        assert!(body.contains("/proof/qa/bundle.json"));
        assert!(body.contains("**Risk:** blast-radius"));
        assert!(body.contains("**Door:** one-way"));
        let legacy = delivery_body(repo, None, &base, "HEAD", None, None, None).unwrap();
        assert!(legacy.contains("**Risk:** not declared"));
        assert!(legacy.contains("behavior run not supplied"));
        assert!(delivery_body(repo, None, "missing-ref", "HEAD", None, None, None).is_err());
    }

    fn two_commit_repo() -> (tempfile::TempDir, String, String) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-q"]).unwrap();
        git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
        git(repo, &["config", "user.name", "Fixture"]).unwrap();
        std::fs::write(repo.join("input.txt"), "before\n").unwrap();
        git(repo, &["add", "."]).unwrap();
        git(repo, &["commit", "-qm", "base"]).unwrap();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(repo.join("input.txt"), "after\n").unwrap();
        git(repo, &["commit", "-qam", "change"]).unwrap();
        let head = git(repo, &["rev-parse", "HEAD"]).unwrap();
        (temp, base, head)
    }

    /// cas-8b0e: the in-band merge-request review carries the worker's
    /// return-contract proof and renders unsupplied fields neutrally.
    #[test]
    fn merge_request_review_shows_contract_proof_and_neutral_gaps() {
        let (temp, base, head) = two_commit_repo();
        let task = Task::new("cas-fixture".into(), "Fixture".into());
        let worker = "status: ready\n\
                      tip: abc on factory/w; worktree: clean\n\
                      ci: 38827289892 green\n\
                      proof: cas-store prompt_queue_store:: 226/226; check: PASS abc\n\
                      deferred: none\n\
                      need: merge";
        let review = merge_request_review(temp.path(), &task, &base, &head, worker)
            .replace(&base, "<BASE>")
            .replace(&head, "<HEAD>");
        insta::assert_snapshot!("merge_request_review_envelope", review);
        for prompt in ["not supplied", "not declared"] {
            // pin: cas-8b0e AC2: the PR-body prompts must never read as failures in the envelope.
            assert!(!review.contains(prompt), "{prompt}: {review}");
        }

        let mut recorded = task.clone();
        recorded.notes = "qa-bundle: /proof/qa/bundle.json".into();
        recorded.risk = vec![cas_types::TaskRisk::BlastRadius];
        recorded.door = Some(cas_types::TaskDoor::OneWay);
        let review = merge_request_review(temp.path(), &recorded, &base, &head, "no contract");
        assert_eq!(contract_evidence("no contract"), Vec::<String>::new());
        assert!(review.contains("/proof/qa/bundle.json"), "{review}");
        assert!(review.contains(&task_risk(&recorded)), "{review}");
        assert!(review.contains(&cas_types::TaskDoor::OneWay.to_string()), "{review}");

        let missing = merge_request_review(temp.path(), &task, "missing-ref", &head, "");
        assert!(missing.contains("missing-ref -> "), "{missing}");
        assert!(missing.contains("Git comparison unavailable"), "{missing}");
        for prompt in ["not supplied", "not declared"] {
            // pin: cas-8b0e AC2: the PR-body prompts must never read as failures in the envelope.
            assert!(!missing.contains(prompt), "{prompt}: {missing}");
        }
    }

    /// cas-8b0e: only exact `proof:` and `ci:` keys count, in any case, and
    /// empty values are skipped.
    #[test]
    fn contract_evidence_reads_only_proof_and_ci_lines() {
        let lines = contract_evidence(
            "  Proof: 3/3 PASS\nci:\nCI: run 7 green\nproofs: nope\nnote: proof: inline",
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("proof: ") && lines[0].ends_with("3/3 PASS"), "{lines:?}");
        assert!(lines[1].starts_with("ci: ") && lines[1].ends_with("run 7 green"), "{lines:?}");
    }
}
