//! Snapshot approval is checked against this task's attributed Git delivery,
//! including after merge. A prose reminder cannot replace this close gate.
use std::path::Path;
use std::process::Command;

fn is_snapshot(path: &str) -> bool {
    path.ends_with(".snap") || path.rsplit('/').next() == Some("opencode_projection.snapshot.json")
}

/// Each note names a file, an actual added/deleted line, and a reason:
/// snapshot-approved: path — +changed line — reason
pub(super) fn rejection(
    repo: &Path,
    base: Option<&str>,
    tip: Option<&str>,
    paths: &[String],
    notes: &str,
    task_id: &str,
    prefix: &str,
) -> Option<String> {
    let snapshots: Vec<_> = paths.iter().filter(|path| is_snapshot(path)).collect();
    if snapshots.is_empty() {
        return None;
    }
    let (Some(base), Some(tip)) = (base, tip) else {
        return Some("SNAPSHOT APPROVAL REQUIRED: cannot resolve the attributed snapshot delivery base/tip; restore the task delivery receipt before close.".into());
    };
    let mut missing = Vec::new();
    for path in snapshots {
        let output = Command::new("git")
            .current_dir(repo)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=0",
                base,
                tip,
                "--",
                path,
            ])
            .output();
        let diff = match output {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            _ => {
                return Some(format!(
                    "SNAPSHOT APPROVAL REQUIRED: cannot inspect {path} in the attributed delivery; restore its Git base/tip before close."
                ));
            }
        };
        let lines: Vec<_> = diff
            .lines()
            .filter(|line| {
                (line.starts_with('+') || line.starts_with('-'))
                    && !line.starts_with("+++")
                    && !line.starts_with("---")
                    || line.starts_with("Binary files ")
            })
            .collect();
        // Paths reported by attribution can include an unchanged snapshot from
        // an earlier commit in the receipt window. Only a real diff needs approval.
        if lines.is_empty() && diff.is_empty() {
            continue;
        }
        let approved = notes.lines().any(|note| {
            let Some((_, body)) = note.split_once("snapshot-approved: ") else {
                return false;
            };
            lines.iter().any(|changed| {
                body.strip_prefix(&format!("{path} — {changed} — "))
                    .is_some_and(|reason| !reason.trim().is_empty())
            })
        });
        if !approved {
            let changed = lines.first().copied().unwrap_or("<binary snapshot change>");
            let approval =
                format!("snapshot-approved: {path} — {changed} — <why this change is correct>");
            missing.push(format!(
                "{path}: no approval for a changed line. Record `{prefix}task action=notes id={task_id} note_type=decision notes={approval:?}`, then retry close."
            ));
        }
    }
    (!missing.is_empty()).then(|| format!("SNAPSHOT APPROVAL REQUIRED\n{}", missing.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn real_git_snapshot_changes_require_file_changed_line_and_reason_even_after_merge() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "fixture"]);
        git(repo, &["config", "user.email", "fixture@example.test"]);
        std::fs::write(repo.join("view.snap"), "before\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["switch", "-qc", "factory/fixture"]);
        std::fs::write(repo.join("view.snap"), "after\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "snapshot change"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        let paths = vec!["view.snap".into()];
        for notes in [
            "",
            "snapshot-approved: other.snap — +after — reviewed",
            "snapshot-approved: view.snap — +wrong — reviewed",
            "snapshot-approved: view.snap — +after — ",
        ] {
            let error = rejection(
                repo,
                Some(&base),
                Some(&tip),
                &paths,
                notes,
                "cas-fixture",
                "mcp__cs__",
            )
            .unwrap();
            assert!(error.contains("mcp__cs__task action=notes id=cas-fixture note_type=decision"));
            assert!(error.contains("view.snap — +after"));
        }
        let notes = "[time] decision snapshot-approved: view.snap — +after — display now includes the new field";
        assert!(
            rejection(
                repo,
                Some(&base),
                Some(&tip),
                &paths,
                notes,
                "cas-fixture",
                "mcp__cs__"
            )
            .is_none()
        );
        git(repo, &["switch", "-q", "main"]);
        git(
            repo,
            &["merge", "--no-ff", "-qm", "merged", "factory/fixture"],
        );
        assert!(
            rejection(
                repo,
                Some(&base),
                Some(&tip),
                &paths,
                "",
                "cas-fixture",
                "mcp__cs__"
            )
            .is_some()
        );
        assert!(
            rejection(
                repo,
                Some(&base),
                Some(&tip),
                &paths,
                notes,
                "cas-fixture",
                "mcp__cs__"
            )
            .is_none()
        );
        assert!(
            rejection(
                repo,
                None,
                Some(&tip),
                &paths,
                notes,
                "cas-fixture",
                "mcp__cs__"
            )
            .is_some()
        );
        assert!(
            rejection(
                repo,
                None,
                None,
                &["regular.json".into()],
                "",
                "cas-fixture",
                "mcp__cs__"
            )
            .is_none()
        );
    }

    #[test]
    fn opencode_projection_is_a_snapshot_but_arbitrary_json_is_not() {
        assert!(is_snapshot(
            "crates/cas-mux/src/opencode_projection.snapshot.json"
        ));
        assert!(is_snapshot("nested/view.snap"));
        assert!(!is_snapshot("nested/view.snapshot.json"));
    }
}
