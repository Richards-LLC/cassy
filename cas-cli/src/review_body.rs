//! Shared review body. Reversibility is descriptive; no policy consumes it.
use anyhow::{Context, Result, bail};
use cas_types::Task;
use std::path::Path;
use std::process::Command;

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

pub(crate) fn task_evidence(task: &Task) -> String {
    let references: Vec<_> = task
        .notes
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
        .collect();
    if references.is_empty() {
        "Evidence not supplied; attach a QA bundle or verify-before-claim base-vs-change run."
            .into()
    } else {
        references.join("\n")
    }
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
}
