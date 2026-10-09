//! Snapshot approval is checked against this task's attributed Git delivery,
//! including after merge. A prose reminder cannot replace this close gate.
use std::path::Path;
use std::process::Command;

fn is_snapshot(path: &str) -> bool {
    path.ends_with(".snap") || path.rsplit('/').next() == Some("opencode_projection.snapshot.json")
}

/// Hash the signed UTF-8 diff line, preserving whitespace and excluding its
/// newline. Path binding and a non-empty rationale remain part of the note.
fn changed_line_digest(changed: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(changed.as_bytes()))
}

/// Each note names a file, an actual changed line (literal or digest), and a reason:
/// snapshot-approved: path — +changed line / sha256:<digest> — reason
pub(super) fn rejection(
    repo: &Path,
    base: Option<&str>,
    tip: Option<&str>,
    paths: &[String],
    notes: &str,
    task_id: &str,
    prefix: &str,
) -> Option<String> {
    let (Some(base), Some(tip)) = (base, tip) else {
        return paths.iter().any(|path| is_snapshot(path)).then(||
            "SNAPSHOT APPROVAL REQUIRED: cannot resolve the attributed snapshot delivery base/tip; restore the task delivery receipt before close.".into());
    };
    // Keep task attribution, but recover both names of a rename: ordinary
    // --name-only output can omit the deleted snapshot's old name.
    let output = Command::new("git")
        .current_dir(repo)
        .args(["diff", "--name-status", "-z", base, tip, "--"])
        .output();
    let names = match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).into_owned(),
        _ => return Some("SNAPSHOT APPROVAL REQUIRED: cannot enumerate the attributed delivery diff; restore its Git base/tip before close.".into()),
    };
    let mut snapshots = std::collections::BTreeSet::new();
    let mut fields = names.split('\0').filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let Some(first) = fields.next() else {
            return Some("SNAPSHOT APPROVAL REQUIRED: malformed Git name-status receipt.".into());
        };
        let second = if status.starts_with('R') || status.starts_with('C') {
            let Some(second) = fields.next() else {
                return Some("SNAPSHOT APPROVAL REQUIRED: malformed Git rename receipt.".into());
            };
            Some(second)
        } else {
            None
        };
        let attributed = paths.is_empty()
            || paths
                .iter()
                .any(|path| path == first || second == Some(path.as_str()));
        if attributed {
            for path in [Some(first), second].into_iter().flatten() {
                if is_snapshot(path) {
                    snapshots.insert(path);
                }
            }
        }
    }
    let mut missing = Vec::new();
    for path in snapshots {
        let output = Command::new("git")
            .current_dir(repo)
            .args([
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
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
                    || line.starts_with("old mode ")
                    || line.starts_with("new mode ")
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
                    .or_else(|| {
                        body.strip_prefix(&format!("{path} — {} — ", changed_line_digest(changed)))
                    })
                    .is_some_and(|reason| !reason.trim().is_empty())
            })
        });
        if !approved {
            // Suggest an added line when there is one: it names the new state.
            let changed = lines
                .iter()
                .find(|line| line.starts_with('+'))
                .or(lines.first())
                .copied()
                .unwrap_or("<binary snapshot change>");
            // Keep short-line guidance familiar; long prompts must fit a task
            // note without weakening the exact changed-line identity.
            let identity = if changed.chars().count() <= 256 {
                changed.to_string()
            } else {
                changed_line_digest(changed)
            };
            let approval =
                format!("snapshot-approved: {path} — {identity} — <why this change is correct>");
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
        git(repo, &["mv", "view.snap", "regular.json"]);
        git(
            repo,
            &["commit", "-qm", "rename snapshot outside extension"],
        );
        let renamed = git(repo, &["rev-parse", "HEAD"]);
        let renamed_paths = vec!["regular.json".into()];
        assert!(
            rejection(
                repo,
                Some(&tip),
                Some(&renamed),
                &renamed_paths,
                "",
                "cas-fixture",
                "mcp__cs__"
            )
            .unwrap()
            .contains("view.snap")
        );
        assert!(rejection(repo, Some(&tip), Some(&renamed), &renamed_paths,
            "snapshot-approved: view.snap — -after — deleted snapshot superseded by regular fixture", "cas-fixture", "mcp__cs__").is_none());
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
    fn cas_b97f_long_snapshot_line_has_a_bounded_actionable_approval() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "fixture"]);
        git(repo, &["config", "user.email", "fixture@example.test"]);
        let path = "opencode_projection.snapshot.json";
        std::fs::write(repo.join(path), "before\n").unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let changed = format!("worker prompt {}", "x".repeat(3100));
        std::fs::write(repo.join(path), format!("{changed}\n")).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "long prompt"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        let paths = vec![path.to_string()];
        let check = |tip: &str, notes: &str| {
            rejection(
                repo,
                Some(&base),
                Some(tip),
                &paths,
                notes,
                "cas-b97f",
                "mcp__cs__",
            )
        };
        let error = check(&tip, "").unwrap();
        let note_json = error
            .split_once("notes=")
            .unwrap()
            .1
            .split_once("`, then retry close.")
            .unwrap()
            .0;
        let note: String = serde_json::from_str(note_json).unwrap();
        let note = note.replace(
            "<why this change is correct>",
            "worker contract matches the reviewed launch behavior",
        );
        let config = crate::config::Config::default();
        crate::mcp::tools::traffic_limits::validate_note_body(
            "decision", &note, &config, "cas-b97f", false, false, None,
        )
        .expect("the refusal's suggested approval must fit the actual note gate");
        assert!(note.chars().count() < 1500);
        assert!(note.contains("sha256:"), "{note}");
        assert!(error.chars().count() < 1500, "refusal must remain bounded");
        assert!(check(&tip, &note).is_none());
        assert!(check(&tip, &note.replace(path, "other.snap")).is_some());
        let no_reason = note.split_once(" — worker contract").unwrap().0.to_string() + " — ";
        assert!(check(&tip, &no_reason).is_some());
        // Change only the end of a >3000-char line: a prefix-only approval
        // would incorrectly accept this new delivery.
        std::fs::write(repo.join(path), format!("{changed}new tail\n")).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "different reviewed state"]);
        let stale_tip = git(repo, &["rev-parse", "HEAD"]);
        assert!(
            check(&stale_tip, &note).is_some(),
            "stale digest must not approve another line"
        );
    }

    #[test]
    fn cas_b98d_one_content_bound_approval_covers_all_snapshot_lines() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "fixture"]);
        git(repo, &["config", "user.email", "fixture@example.test"]);
        let path = "email-queue.locale.spec.ts.snap";
        let before: String = (0..20)
            .map(|i| format!("email {i}: {}\n", "x".repeat(4256)))
            .collect();
        std::fs::write(repo.join(path), &before).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "base emails"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let after = before.replace("\n", "<span data-notification-preferences>Manage preferences</span>\n");
        std::fs::write(repo.join(path), &after).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "shared footer"]);
        let tip = git(repo, &["rev-parse", "HEAD"]);
        let check = |tip: &str, notes: &str| {
            rejection(repo, Some(&base), Some(tip), &[path.into()], notes, "cas-b98d", "mcp__cs__")
        };
        let identity = format!("file-sha256:{:x}", Sha256::digest(after.as_bytes()));
        let error = check(&tip, "").unwrap();
        assert!(error.contains(&identity), "guidance must bind the complete snapshot: {error}");
        let note = format!("snapshot-approved: {path} — {identity} — reviewed shared preferences footer in all 20 emails");
        crate::mcp::tools::traffic_limits::validate_note_body(
            "decision", &note, &crate::config::Config::default(), "cas-b98d", false, false, None,
        ).unwrap();
        assert!(check(&tip, &note).is_none(), "one note must cover the file");
        assert!(check(&tip, &note.replace(path, "other.snap")).is_some());
        assert!(check(&tip, &format!("snapshot-approved: {path} — {identity} — ")).is_some());
        assert!(check(&tip, &note.replace("file-sha256:", "sha256:")).is_some());
        // This leaves 19 previously reviewed lines in the diff. A line-level
        // digest cannot bind approval of the whole file after this edit.
        std::fs::write(repo.join(path), after.replacen("email 0", "unreviewed email 0", 1)).unwrap();
        git(repo, &["add", "."]);
        git(repo, &["commit", "-qm", "post approval edit"]);
        let edited_tip = git(repo, &["rev-parse", "HEAD"]);
        assert!(check(&edited_tip, &note).is_some(), "post-approval edit must invalidate the file approval");
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
