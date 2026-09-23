//! Independent QA and polish pass (cas-619f): eligibility and the QA work
//! item. Storage, the no-self-review rule and round accounting live in
//! `cas_store::qa_pass_store`; this module decides *whether* a delivery
//! needs a pass and what the reviewer is told to do.
//!
//! Design: `docs/qa/independent-qa-pass.md`.

use std::path::Path;
use std::process::Command;

use cas_types::{QaPass, Task, TaskType};

use crate::config::QaConfig;

/// Label carried by every Cassy-created QA work item.
pub const QA_PASS_LABEL: &str = "qa-pass";

/// Why a delivery needs the independent pass (empty when it does not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QaEligibility {
    pub reasons: Vec<String>,
}

impl QaEligibility {
    pub fn is_eligible(&self) -> bool {
        !self.reasons.is_empty()
    }
}

/// Paths that never make a delivery user-facing: documentation, tests and
/// test harnesses, fixtures, and CI configuration. A delivery made only of
/// these is never gated, whatever its demo_statement says.
pub fn is_non_surface_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let in_dir = |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    ["docs", "doc", "tests", "test", "__tests__", "e2e", "fixtures", "testdata", ".github", ".circleci", ".gitlab", ".buildkite"]
        .iter()
        .any(|dir| in_dir(dir))
        || [".md", ".mdx", ".rst", ".adoc"].iter().any(|ext| file.ends_with(ext))
        || file.contains(".test.")
        || file.contains(".spec.")
        || file.ends_with("_test.rs")
        || file.ends_with("_tests.rs")
        || file.ends_with("_test.go")
        || (file.starts_with("test_") && file.ends_with(".py"))
        || file.starts_with("playwright.config.")
        || file.starts_with("vitest.config.")
        || file.starts_with("jest.config.")
        || file == ".gitlab-ci.yml"
        || lower.starts_with("scripts/test-")
}

/// Decide whether a parked delivery needs an independent QA pass.
///
/// `changed_paths` is the delivery diff when Cassy could compute it.
/// `journeys` are the catalog journeys (`scripts/journeys-for-diff.py`) its
/// surface paths touch. Eligible when the diff touches a user-facing
/// surface (a catalog journey or a `qa.user_facing_paths` glob) or the task
/// carries a demo_statement — except that a known diff made only of docs,
/// tests or CI is never gated. Epics and QA work items are never eligible.
pub fn delivery_eligibility(
    task: &Task,
    qa: &QaConfig,
    changed_paths: Option<&[String]>,
    journeys: &[String],
) -> QaEligibility {
    let mut reasons = Vec::new();
    if !qa.independent_pass
        || task.task_type == TaskType::Epic
        || task.labels.iter().any(|label| label == QA_PASS_LABEL)
    {
        return QaEligibility { reasons };
    }
    let surface_paths: Vec<String> = changed_paths
        .unwrap_or(&[])
        .iter()
        .filter(|path| !is_non_surface_path(path))
        .cloned()
        .collect();
    if let Some(paths) = changed_paths
        && !paths.is_empty()
        && surface_paths.is_empty()
    {
        return QaEligibility { reasons };
    }
    if !journeys.is_empty() {
        reasons.push(format!("journeys:{}", journeys.join(",")));
    }
    if let Some((path, glob)) = first_user_facing_path(&surface_paths, &qa.user_facing_paths) {
        reasons.push(format!("path:{path} ({glob})"));
    }
    if !task.demo_statement.trim().is_empty() {
        reasons.push("demo_statement".to_string());
    }
    QaEligibility { reasons }
}

/// Catalog journeys the given paths touch, via the project's own
/// `scripts/journeys-for-diff.py --paths` (cas-9be7). Empty when the project
/// has no catalog or the helper fails; config globs still apply then.
pub fn catalog_journeys_for(repo: &Path, paths: &[String]) -> Vec<String> {
    let script = repo.join("scripts/journeys-for-diff.py");
    let surface: Vec<&String> = paths.iter().filter(|path| !is_non_surface_path(path)).collect();
    if surface.is_empty() || !script.is_file() || !repo.join("docs/qa/journeys.md").is_file() {
        return Vec::new();
    }
    let output = Command::new("python3")
        .arg(&script)
        .arg("--paths")
        .args(surface)
        .env("CAS_JOURNEYS_ROOT", repo)
        .current_dir(repo)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.get("journeys").and_then(|j| j.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|journey| journey.get("id").and_then(|id| id.as_str()).map(str::to_string))
        .collect()
}

/// Whether a task is bound by the merge gates: the park (which saw the
/// delivery diff) found it user-facing and opened a round. Deciding from the
/// recorded round keeps docs/test/CI-only deliveries ungated even when they
/// carry a demo_statement.
pub fn gate_applies(task: &Task, qa: &QaConfig, passes: &[QaPass]) -> bool {
    qa.independent_pass
        && task.task_type != TaskType::Epic
        && !task.labels.iter().any(|label| label == QA_PASS_LABEL)
        && !passes.is_empty()
}

/// Merge gate for one exact tip: a passed or waived round must cover `head`.
/// Returns the refusal text when the merge must wait.
pub fn merge_gate(task: &Task, qa: &QaConfig, passes: &[QaPass], head: &str) -> Result<(), String> {
    if !gate_applies(task, qa, passes) {
        return Ok(());
    }
    if passes
        .iter()
        .any(|pass| pass.bound_head == head && pass.state.satisfies_gate())
    {
        return Ok(());
    }
    let head8 = &head[..head.len().min(8)];
    let status = match passes.first() {
        Some(latest) => format!(
            "latest round {} ({}) is {} for @{}{}",
            latest.round,
            latest.id,
            latest.state,
            latest.head8(),
            latest
                .qa_task_id
                .as_deref()
                .map(|qa_task| format!(", QA task {qa_task}"))
                .unwrap_or_default()
        ),
        None => "no round has been dispatched yet (the worker's close dispatches it)".to_string(),
    };
    Err(format!(
        "INDEPENDENT QA REQUIRED before {task} merges: no passed or waived QA round covers @{head8}; {status}. \
         Spawn a reviewer who is not the implementer (spawn_workers lane=taste task_id=<QA task>), \
         or waive with a logged reason: verification action=qa_waive task_id={task} summary=\"...\". \
         Check with: verification action=qa_status task_id={task}",
        task = task.id,
    ))
}

/// First changed path matching a configured glob, with the glob it matched.
pub fn first_user_facing_path<'a>(
    changed_paths: &'a [String],
    globs: &'a [String],
) -> Option<(&'a str, &'a str)> {
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let patterns: Vec<(glob::Pattern, &str)> = globs
        .iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            glob::Pattern::new(raw).ok().map(|pattern| (pattern, raw))
        })
        .collect();
    changed_paths.iter().find_map(|path| {
        patterns.iter().find_map(|(pattern, raw)| {
            // `**/x` must also match `x` at the repository root.
            let root_match = raw
                .strip_prefix("**/")
                .and_then(|rest| glob::Pattern::new(rest).ok())
                .is_some_and(|rest| rest.matches_with(path, options));
            (pattern.matches_with(path, options) || root_match).then_some((path.as_str(), *raw))
        })
    })
}

/// Factory branches a shell command would `git merge` (cas-619f pre-tool
/// guard). `--abort`/`--continue`/`--quit` statements merge nothing new.
pub fn factory_branches_merged_by(command: &str) -> Vec<String> {
    let mut branches = Vec::new();
    for statement in command.split(['\n', ';', '|', '&']) {
        let tokens: Vec<&str> = statement
            .split_whitespace()
            .map(|token| token.trim_matches(|ch: char| matches!(ch, '\'' | '"' | '`' | '(' | ')')))
            .collect();
        let Some(git) = tokens.iter().position(|token| *token == "git" || token.ends_with("/git"))
        else {
            continue;
        };
        let Some(merge) = tokens[git + 1..].iter().position(|token| *token == "merge") else {
            continue;
        };
        let args = &tokens[git + 1 + merge + 1..];
        if args
            .iter()
            .any(|arg| matches!(*arg, "--abort" | "--continue" | "--quit"))
        {
            continue;
        }
        for arg in args {
            let name = arg
                .trim_start_matches("refs/remotes/")
                .trim_start_matches("refs/heads/")
                .trim_start_matches("origin/");
            if let Some(worker) = name.strip_prefix("factory/")
                && !worker.is_empty()
                && !branches.iter().any(|branch: &String| branch == name)
            {
                branches.push(name.to_string());
            }
        }
    }
    branches
}

/// Supervisor pre-tool guard: the refusal when a `git merge` would integrate
/// a parked user-facing delivery whose current tip has no passed or waived
/// independent QA round. Fails open (None) when Cassy state is unreadable —
/// the close backstop still refuses such a task later.
pub fn supervisor_merge_refusal(cas_root: &Path, cwd: &Path, command: &str) -> Option<String> {
    let branches = factory_branches_merged_by(command);
    if branches.is_empty() {
        return None;
    }
    let config = crate::config::Config::load(cas_root).ok()?;
    let qa = config.qa();
    if !qa.independent_pass {
        return None;
    }
    let task_store = crate::store::open_task_store(cas_root).ok()?;
    let parked = task_store.list(Some(cas_types::TaskStatus::AwaitingMerge)).ok()?;
    for branch in branches {
        let worker = branch.trim_start_matches("factory/");
        for task in parked.iter().filter(|task| {
            task.deliverables.parked_branch.as_deref() == Some(branch.as_str())
                || task.assignee.as_deref() == Some(worker)
        }) {
            let passes = cas_store::list_qa_passes(cas_root, &task.id).unwrap_or_default();
            if !gate_applies(task, &qa, &passes) {
                continue;
            }
            let head = Command::new("git")
                .args(["rev-parse", "--verify", &branch])
                .current_dir(cwd)
                .output()
                .ok()
                .filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .filter(|sha| !sha.is_empty());
            let refusal = match head {
                Some(head) => merge_gate(task, &qa, &passes, &head).err(),
                None => Some(format!(
                    "INDEPENDENT QA REQUIRED before {} merges, and {branch} does not resolve here.",
                    task.id
                )),
            };
            if let Some(refusal) = refusal {
                return Some(format!("🚫 {refusal}"));
            }
        }
    }
    None
}

/// Paths the delivery changes: `merge-base(parent, branch)..branch`.
pub fn changed_paths_for_delivery(
    repo: &Path,
    parent_branch: &str,
    branch: &str,
) -> Result<Vec<String>, String> {
    let base = Command::new("git")
        .args(["merge-base", parent_branch, branch])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git merge-base failed to start: {error}"))?;
    if !base.status.success() {
        return Err(format!(
            "git merge-base {parent_branch} {branch} failed: {}",
            String::from_utf8_lossy(&base.stderr).trim()
        ));
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
    let diff = Command::new("git")
        .args(["diff", "--name-only", &format!("{base}..{branch}")])
        .current_dir(repo)
        .output()
        .map_err(|error| format!("git diff failed to start: {error}"))?;
    if !diff.status.success() {
        return Err(format!(
            "git diff --name-only {base}..{branch} failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&diff.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

/// Paths a delivery brought into `target` after it merged: the change set of
/// the first merge on the ancestry path from `head` to `target`, or
/// `merge-base..head` when the tip has not merged. `None` when neither can
/// be computed (fast-forward merges carry no merge commit).
pub fn integrated_paths(repo: &Path, head: &str, target: &str) -> Option<Vec<String>> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").args(args).current_dir(repo).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let merged = Command::new("git")
        .args(["merge-base", "--is-ancestor", head, target])
        .current_dir(repo)
        .status()
        .is_ok_and(|status| status.success());
    let (from, to) = if merged {
        let merges = git(&[
            "rev-list",
            "--ancestry-path",
            "--merges",
            "--reverse",
            &format!("{head}..{target}"),
        ])?;
        let merge = merges.lines().next()?.trim().to_string();
        (format!("{merge}^1"), merge)
    } else {
        (git(&["merge-base", target, head])?, head.to_string())
    };
    let diff = git(&["diff", "--name-only", &from, &to])?;
    Some(
        diff.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

/// `epic_status` section listing each child's latest independent QA round
/// (cas-619f). Waivers show who waived and why. Empty when no child has one.
pub fn render_epic_qa_section(cas_root: &Path, children: &[Task]) -> String {
    let mut lines = Vec::new();
    for child in children {
        let Ok(passes) = cas_store::list_qa_passes(cas_root, &child.id) else {
            continue;
        };
        let Some(latest) = passes.first() else {
            continue;
        };
        let detail = match latest.state {
            cas_types::QaPassState::Waived => format!(
                "WAIVED by {} — {}",
                latest.issuer_agent_id.as_deref().unwrap_or("supervisor"),
                latest.summary.as_deref().unwrap_or("(no reason recorded)")
            ),
            cas_types::QaPassState::Passed | cas_types::QaPassState::Failed => format!(
                "{} by {}{}",
                latest.state,
                latest.reviewer_agent_id.as_deref().unwrap_or("-"),
                latest
                    .ledger_path
                    .as_deref()
                    .map(|ledger| format!(" — {ledger}"))
                    .unwrap_or_default()
            ),
            state => format!(
                "{state}{} — QA task {}",
                latest
                    .reviewer_agent_id
                    .as_deref()
                    .map(|reviewer| format!(" by {reviewer}"))
                    .unwrap_or_default(),
                latest.qa_task_id.as_deref().unwrap_or("-")
            ),
        };
        lines.push(format!(
            "- {} round {} @{}: {detail}",
            child.id,
            latest.round,
            latest.head8()
        ));
    }
    if lines.is_empty() {
        return String::new();
    }
    format!("\n\nIndependent QA:\n{}\n", lines.join("\n"))
}

/// Title of the QA work item for one round.
pub fn qa_task_title(delivery: &Task, pass: &QaPass) -> String {
    let mut title = delivery.title.trim().to_string();
    if title.chars().count() > 80 {
        title = title.chars().take(77).collect::<String>() + "...";
    }
    format!(
        "QA pass (round {}): {title} @{}",
        pass.round,
        pass.head8()
    )
}

/// Ledger directory for one round, under the delivery's artifacts dir.
///
/// Deliberately beside, not inside, the implementer's own cas-qa-craft
/// bundle (`<task>/qa/`): the close-time evidence gate reads that directory
/// as the implementer's evidence, and an independent round must never be
/// mistaken for it (or vice versa).
pub fn round_dir(artifacts_root: &Path, pass: &QaPass) -> std::path::PathBuf {
    artifacts_root
        .join(&pass.task_id)
        .join("independent-qa")
        .join(format!("round-{}", pass.round))
}

/// Brief the reviewer reads when it starts the QA work item.
pub fn qa_task_description(
    delivery: &Task,
    pass: &QaPass,
    reasons: &[String],
    ledger_dir: &Path,
    parent_branch: &str,
) -> String {
    let demo = if delivery.demo_statement.trim().is_empty() {
        "(none — derive the user goal from the delivery title and diff)".to_string()
    } else {
        delivery.demo_statement.trim().to_string()
    };
    format!(
        "Independent QA and polish pass for delivery {task} — round {round}.\n\n\
         You are NOT the implementer ({implementer}); Cassy refuses the implementer here. \
         Review the running product, never edit the delivery. Follow the cas-qa-craft skill, \
         section \"Independent pass\".\n\n\
         - Delivery: {task} — {title}\n\
         - Demo statement: {demo}\n\
         - Branch: {branch} at {head} (review exactly this tip; base: {parent})\n\
         - Why this pass: {reasons}\n\
         - Ledger: {ledger}/LEDGER.md (evidence beside it)\n\
         - Deadline: {deadline} (pass {pass_id})\n\n\
         Steps: build and serve {head}; walk the journeys the diff touches \
         (scripts/journeys-for-diff.py {parent} {head}) plus the demo statement; \
         walk the adjacent paths (empty, loading, error, long content, phone 390px, dark, \
         keyboard-only, reduced motion); run visual-qa.mjs --strict and score the \
         cas-ui-craft rubric with desktop+phone, light+dark screenshots. Every finding \
         cites a trace action and a screenshot.\n\n\
         Record the verdict with: mcp__cas__verification action=qa_record task_id={task} \
         status=approved|rejected summary=\"...\" issues='[...]' ledger_path={ledger}/LEDGER.md \
         — a rejection sends {task} back to its implementer with your ledger.",
        task = delivery.id,
        round = pass.round,
        implementer = pass.implementer_agent_id,
        title = delivery.title.trim(),
        branch = pass.branch,
        head = pass.bound_head,
        parent = parent_branch,
        reasons = reasons.join(", "),
        ledger = ledger_dir.display(),
        deadline = pass.deadline_at.to_rfc3339(),
        pass_id = pass.id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task::new("cas-ui1".to_string(), "Reply composer".to_string())
    }

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn eligibility_from_demo_journeys_or_surface_path() {
        let qa = QaConfig::default();
        let backend = paths(&["cas-cli/src/lib.rs"]);
        assert!(!delivery_eligibility(&task(), &qa, Some(&backend), &[]).is_eligible());

        let mut demo = task();
        demo.demo_statement = "Type a reply and see it land".to_string();
        assert_eq!(
            delivery_eligibility(&demo, &qa, Some(&backend), &[]).reasons,
            vec!["demo_statement".to_string()]
        );
        // Unknown diff: the demo statement alone still qualifies.
        assert!(delivery_eligibility(&demo, &qa, None, &[]).is_eligible());

        let css = paths(&["docs/x.md", "hub-web/src/a.css"]);
        assert_eq!(
            delivery_eligibility(&task(), &qa, Some(&css), &[]).reasons,
            vec!["path:hub-web/src/a.css (**/*.css)".to_string()]
        );
        let ts = paths(&["hub-web/src/composer-markup.ts"]);
        assert_eq!(
            delivery_eligibility(&task(), &qa, Some(&ts), &["HUB-J5".to_string()]).reasons,
            vec!["journeys:HUB-J5".to_string()]
        );
    }

    #[test]
    fn labels_alone_do_not_gate() {
        let qa = QaConfig::default();
        let mut labelled = task();
        labelled.labels = vec!["ui".to_string(), "hub".to_string()];
        let backend = paths(&["cas-cli/src/lib.rs"]);
        assert!(!delivery_eligibility(&labelled, &qa, Some(&backend), &[]).is_eligible());
    }

    #[test]
    fn docs_test_and_ci_only_deliveries_are_never_gated() {
        let qa = QaConfig::default();
        let mut demo = task();
        demo.demo_statement = "Reply lands".to_string();
        for only in [
            vec!["docs/qa/journeys.md", "README.md"],
            vec!["hub-web/e2e/journeys/reply.journey.spec.ts", "hub-web/playwright.config.ts"],
            vec!["cas-cli/tests/cli_test.rs", "crates/x/src/foo_tests.rs"],
            vec![".github/workflows/ci.yml", "scripts/test-ci-test-tiers.sh"],
            vec!["hub-web/src/composer.test.ts", "fixtures/hub/a.json", "hub-web/DESIGN.md"],
        ] {
            let changed = paths(&only);
            assert!(
                !delivery_eligibility(&demo, &qa, Some(&changed), &["HUB-J1".to_string()])
                    .is_eligible(),
                "{only:?} must never be gated"
            );
        }
        // One real surface file among them is enough.
        let mixed = paths(&["docs/a.md", "hub-web/src/styles.css"]);
        assert!(delivery_eligibility(&task(), &qa, Some(&mixed), &[]).is_eligible());
    }

    #[test]
    fn epics_qa_items_and_disabled_config_are_never_eligible() {
        let mut qa = QaConfig::default();
        let css = paths(&["a.css"]);
        let mut epic = task();
        epic.task_type = TaskType::Epic;
        epic.demo_statement = "demo".to_string();
        assert!(!delivery_eligibility(&epic, &qa, Some(&css), &[]).is_eligible());

        let mut qa_item = task();
        qa_item.labels = vec![QA_PASS_LABEL.to_string()];
        assert!(!delivery_eligibility(&qa_item, &qa, Some(&css), &[]).is_eligible());

        qa.independent_pass = false;
        assert!(!delivery_eligibility(&task(), &qa, Some(&css), &[]).is_eligible());
    }

    fn pass(head: &str, state: cas_types::QaPassState) -> QaPass {
        let now = chrono::Utc::now();
        QaPass {
            id: format!("qapass-{head}"),
            task_id: "cas-ui1".to_string(),
            round: 1,
            implementer_agent_id: "impl".to_string(),
            branch: "factory/impl".to_string(),
            bound_head: head.to_string(),
            qa_task_id: Some("cas-qa1".to_string()),
            reviewer_agent_id: None,
            state,
            summary: None,
            issues_json: None,
            ledger_path: None,
            issuer_agent_id: None,
            requested_at: now,
            deadline_at: now,
            resolved_at: None,
        }
    }

    #[test]
    fn merge_gate_requires_a_satisfying_round_for_the_exact_head() {
        use cas_types::QaPassState::*;
        let qa = QaConfig::default();
        let mut demo = task();
        demo.demo_statement = "Reply lands".to_string();

        // No recorded round: the park judged it not user-facing (or it
        // predates the gate), so merge is not blocked here.
        assert!(merge_gate(&demo, &qa, &[], "aaaa1111bbbb").is_ok());

        let pending = merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Pending)], "aaaa1111bbbb")
            .unwrap_err();
        assert!(pending.contains("is pending"), "{pending}");
        assert!(pending.contains("cas-qa1"), "{pending}");

        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Passed)], "aaaa1111bbbb").is_ok());
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Waived)], "aaaa1111bbbb").is_ok());
        // A pass for an older tip does not cover new commits.
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Passed)], "cccc2222").is_err());
        assert!(merge_gate(&demo, &qa, &[pass("aaaa1111bbbb", Failed)], "aaaa1111bbbb").is_err());
    }

    #[test]
    fn merge_gate_binds_only_recorded_rounds() {
        let qa = QaConfig::default();
        assert!(merge_gate(&task(), &qa, &[], "aaaa1111").is_ok());
        // A path-eligible delivery is bound once the park recorded a round.
        assert!(
            merge_gate(
                &task(),
                &qa,
                &[pass("aaaa1111", cas_types::QaPassState::Pending)],
                "aaaa1111"
            )
            .is_err()
        );
        let mut disabled = QaConfig::default();
        disabled.independent_pass = false;
        assert!(
            merge_gate(
                &task(),
                &disabled,
                &[pass("aaaa1111", cas_types::QaPassState::Pending)],
                "aaaa1111"
            )
            .is_ok()
        );
    }

    #[test]
    fn merge_commands_name_the_factory_branches_they_integrate() {
        assert_eq!(
            factory_branches_merged_by("git merge --no-ff factory/zealous-cheetah-52"),
            vec!["factory/zealous-cheetah-52".to_string()]
        );
        assert_eq!(
            factory_branches_merged_by(
                "cd /repo && git fetch && git merge --no-edit origin/factory/a-1 factory/b-2"
            ),
            vec!["factory/a-1".to_string(), "factory/b-2".to_string()]
        );
        assert!(factory_branches_merged_by("git merge --abort").is_empty());
        assert!(factory_branches_merged_by("git merge epic/x").is_empty());
        assert!(factory_branches_merged_by("git log factory/a-1").is_empty());
        assert!(factory_branches_merged_by("echo factory/a-1 merge").is_empty());
    }

    #[test]
    fn globs_match_nested_and_root_paths_only() {
        let globs = paths(&["**/*.css", "hub-web/**"]);
        assert_eq!(
            first_user_facing_path(&paths(&["styles.css"]), &globs),
            Some(("styles.css", "**/*.css"))
        );
        assert_eq!(
            first_user_facing_path(&paths(&["hub-web/src/main.ts"]), &globs),
            Some(("hub-web/src/main.ts", "hub-web/**"))
        );
        assert_eq!(first_user_facing_path(&paths(&["cas-cli/src/lib.rs"]), &globs), None);
        assert_eq!(first_user_facing_path(&paths(&["a.css"]), &[]), None);
    }
}
