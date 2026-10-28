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
    let (summary, details) = match delivery_summary(repo, base, head) {
        // cas-5a52: informational only; nothing gates on the delta.
        Ok(summary) => match line_delta(repo, base, head) {
            Ok(delta) => (format!("{summary}\n\n{delta}"), String::new()),
            Err(_) => (summary, String::new()),
        },
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

/// Preserve copied proof once, with the same immutable file used by local reads
/// and the published artifact. Publication errors leave the caller's full text
/// in-band; a missing evidence store must never hide evidence.
pub(crate) fn relay_evidence_manifest(
    cas_root: &Path,
    task_id: &str,
    body: &str,
) -> Result<String> {
    use crate::artifacts::{PublishContext, paths::PublishRoots};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let root =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let roots = PublishRoots::new(cas_root, &root, task_id);
    std::fs::create_dir_all(&roots.task_artifacts_dir)?;
    let name = format!("relay-evidence-{:x}.md", Sha256::digest(body.as_bytes()));
    let path = roots.task_artifacts_dir.join(&name);
    let mut file = tempfile::NamedTempFile::new_in(&roots.task_artifacts_dir)?;
    file.write_all(body.as_bytes())?;
    match file.persist_noclobber(&path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::ensure!(
                std::fs::read(&path)? == body.as_bytes(),
                "existing manifest does not match its evidence"
            );
        }
        Err(error) => return Err(error.error.into()),
    }
    let store = cas_store::SqliteArtifactStore::open(cas_root)?;
    let artifact = match store
        .list_for_task(task_id)?
        .into_iter()
        .find(|artifact| artifact.name == name)
    {
        Some(artifact) => artifact,
        None => {
            crate::artifacts::publish(
                &PublishContext {
                    store: &store,
                    roots,
                    task_id: task_id.into(),
                    cloud: crate::artifacts::cloud_client(cas_root),
                },
                &path,
            )?
            .artifact
        }
    };
    let read = if artifact.status == "committed" {
        format!(
            "Evidence manifest: artifact action=fetch task_id={task_id} id={}",
            artifact.id
        )
    } else {
        format!(
            "Evidence manifest: {} (local). Read file {}",
            artifact.id,
            path.display()
        )
    };
    Ok(read)
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

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Test,
    Source,
    Docs,
}

/// Test files by path convention across the languages Cassy projects use.
fn path_kind(path: &str) -> LineKind {
    let name = path.rsplit('/').next().unwrap_or(path);
    let test_dir = path.split('/').any(|segment| {
        matches!(
            segment,
            "tests" | "test" | "__tests__" | "e2e" | "snapshots"
        )
    });
    let test_name = name == "tests.rs"
        || [".test.", ".spec.", "_test.", "_tests."]
            .iter()
            .any(|marker| name.contains(marker))
        || (name.starts_with("test_") && name.ends_with(".py"));
    if test_dir || test_name {
        LineKind::Test
    } else if name.ends_with(".md") {
        LineKind::Docs
    } else {
        LineKind::Source
    }
}

/// First line (1-based) of an inline Rust `#[cfg(test)] mod name {` block;
/// everything from it on counts as test code.
fn inline_test_start(text: &str) -> Option<usize> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines.iter().enumerate().find_map(|(index, line)| {
        (*line == "#[cfg(test)]"
            && lines[index + 1..]
                .iter()
                .find(|next| !next.starts_with("#["))
                .is_some_and(|next| next.starts_with("mod ") && next.ends_with('{')))
        .then_some(index + 1)
    })
}

/// One side of a changed file: its path kind and any inline test module.
#[derive(Clone, Copy)]
struct FileLines {
    kind: LineKind,
    inline_tests_from: Option<usize>,
}

impl FileLines {
    fn read(repo: &Path, rev: &str, path: &str) -> Self {
        let kind = path_kind(path);
        let inline_tests_from = (kind == LineKind::Source && path.ends_with(".rs"))
            .then(|| git(repo, &["show", &format!("{rev}:{path}")]).ok())
            .flatten()
            .and_then(|text| inline_test_start(&text));
        Self {
            kind,
            inline_tests_from,
        }
    }

    fn kind_at(self, line: usize) -> LineKind {
        match self.inline_tests_from {
            Some(start) if line >= start => LineKind::Test,
            _ => self.kind,
        }
    }
}

/// Added and removed lines of `base...head`, split into test, source and docs.
fn line_delta(repo: &Path, base: &str, head: &str) -> Result<String> {
    let (base_sha, head_sha) = resolve_pair(repo, base, head)?;
    let old_rev = git(repo, &["merge-base", &base_sha, &head_sha])?;
    let range = format!("{old_rev}..{head_sha}");
    let diff = git(
        repo,
        &[
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            &range,
        ],
    )?;
    let unknown = FileLines {
        kind: LineKind::Source,
        inline_tests_from: None,
    };
    // [added, removed] per kind, indexed Test, Source, Docs.
    let mut counts = [[0usize; 2]; 3];
    let (mut old, mut new) = (unknown, unknown);
    let (mut old_line, mut new_line, mut in_hunk) = (0usize, 0usize, false);
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            (old, new, in_hunk) = (unknown, unknown, false);
        } else if !in_hunk && let Some(path) = line.strip_prefix("--- a/") {
            old = FileLines::read(repo, &old_rev, path.trim_end_matches('\t'));
        } else if !in_hunk && let Some(path) = line.strip_prefix("+++ b/") {
            new = FileLines::read(repo, &head_sha, path.trim_end_matches('\t'));
        } else if let Some(ranges) = line.strip_prefix("@@ -") {
            // `@@ -old[,n] +new[,n] @@`
            let start = |text: &str| text.split(',').next()?.parse().ok();
            let mut ranges = ranges.split(' ');
            old_line = ranges.next().and_then(start).unwrap_or(0);
            new_line = ranges
                .next()
                .and_then(|text| start(text.strip_prefix('+')?))
                .unwrap_or(0);
            in_hunk = true;
        } else if in_hunk && line.starts_with('+') {
            counts[new.kind_at(new_line) as usize][0] += 1;
            new_line += 1;
        } else if in_hunk && line.starts_with('-') {
            counts[old.kind_at(old_line) as usize][1] += 1;
            old_line += 1;
        }
    }
    let [test, source, docs] = counts.map(|[added, removed]| format!("+{added} -{removed}"));
    Ok(format!(
        "**Lines:** test {test} · source {source} · docs {docs}"
    ))
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
        assert!(
            review.contains(&cas_types::TaskDoor::OneWay.to_string()),
            "{review}"
        );

        let missing = merge_request_review(temp.path(), &task, "missing-ref", &head, "");
        assert!(missing.contains("missing-ref -> "), "{missing}");
        assert!(missing.contains("Git comparison unavailable"), "{missing}");
        for prompt in ["not supplied", "not declared"] {
            // pin: cas-8b0e AC2: the PR-body prompts must never read as failures in the envelope.
            assert!(!missing.contains(prompt), "{prompt}: {missing}");
        }
    }

    /// cas-5a52: the merge request shows test and source line deltas,
    /// counting an inline `#[cfg(test)] mod` as test code.
    #[test]
    fn merge_request_review_splits_lines_into_test_source_and_docs() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        let write = |path: &str, text: &str| {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(repo, &["init", "-q"]).unwrap();
        git(repo, &["config", "user.email", "fixture@example.test"]).unwrap();
        git(repo, &["config", "user.name", "Fixture"]).unwrap();
        write(
            "src/lib.rs",
            "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n",
        );
        write("README.md", "x\n");
        git(repo, &["add", "."]).unwrap();
        git(repo, &["commit", "-qm", "base"]).unwrap();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        write(
            "src/lib.rs",
            "fn a() {}\nfn b() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn u() {}\n}\n",
        );
        write("tests/it.rs", "#[test]\nfn it() {}\n");
        write("README.md", "y\n");
        git(repo, &["add", "."]).unwrap();
        git(repo, &["commit", "-qm", "change"]).unwrap();

        let review = merge_request_review(
            repo,
            &Task::new("cas-fixture".into(), "Fixture".into()),
            &base,
            "HEAD",
            "",
        );
        assert!(
            review.contains("**Lines:** test +3 -1 · source +1 -0 · docs +1 -1"),
            "{review}"
        );
    }

    /// cas-8b0e: only exact `proof:` and `ci:` keys count, in any case, and
    /// empty values are skipped.
    #[test]
    fn contract_evidence_reads_only_proof_and_ci_lines() {
        let lines = contract_evidence(
            "  Proof: 3/3 PASS\nci:\nCI: run 7 green\nproofs: nope\nnote: proof: inline",
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("proof: ") && lines[0].ends_with("3/3 PASS"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("ci: ") && lines[1].ends_with("run 7 green"),
            "{lines:?}"
        );
    }
}
