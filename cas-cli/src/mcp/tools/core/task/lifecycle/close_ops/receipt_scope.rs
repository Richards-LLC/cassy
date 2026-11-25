//! cas-b38a: a blast-radius close's proof scope, derived from the delivered
//! diff and the capped runner's test receipts.
//!
//! The blast-radius gate compared the delivery's source modules only with the
//! task's declared `proof_targets`. Workers declare those when the task is
//! created, before they know which modules the change will touch, so nearly
//! every close was refused and needed a supervisor `proof_scope_fix` that
//! merely listed the delivered modules. The worker had already run targeted
//! tests in those crates; the capped runner records each passing run as a
//! receipt bound to the commit it tested.
//!
//! A delivered module is therefore covered when its crate has a passing test
//! receipt at the delivered head. Only a module whose crate was not tested
//! at all stays uncovered, which keeps the blast-radius intent: every crate a
//! delivery reaches into has been exercised at that exact commit.

use std::collections::BTreeMap;
use std::path::Path;

use cas_types::{Task, TaskRisk};

use super::{SourceModule, changed_source_modules, proof_target_matches_module};
use crate::factory_worker_check::{PassingTests, stability, widened::{self as widen, Scope}};

/// The modules a delivery's test receipts cover beyond its declared targets.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ReceiptScope {
    /// Crate-qualified modules (`cas-store::prompt_queue_store`), each a
    /// proof target the declared scope lacked.
    pub(super) modules: Vec<String>,
    /// The tested packages whose receipts cover them.
    pub(super) packages: Vec<String>,
}

impl ReceiptScope {
    pub(super) fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// The task as the gate should judge it: its declared targets plus the
    /// modules the receipts cover. The stored task is not changed.
    pub(super) fn widen(&self, task: &Task) -> Task {
        let mut widened = task.clone();
        for module in &self.modules {
            if !widened.proof_targets.contains(module) {
                widened.proof_targets.push(module.clone());
            }
        }
        widened
    }

    /// The decision recorded on the task, so the reviewer sees what was
    /// accepted and why no scope fix was asked for.
    pub(super) fn note(&self, head: &str) -> String {
        format!(
            "PROOF SCOPE derived from the delivered diff (cas-b38a): {} covered by passing worker test receipts at {head} for {}; no proof_scope_fix needed.",
            self.modules.join(", "),
            self.packages.join(", "),
        )
    }
}

/// The task the blast-radius gate judges at this close, and the decision
/// note to record, when the worker's test receipts at the delivered head
/// cover modules its declared targets miss. `None` leaves the gate as it was.
#[cfg(test)]
pub(super) fn for_close(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    delivered_tip: Option<&str>,
) -> Option<(Task, String)> {
    let head = delivered_tip?;
    let tested = crate::factory_worker_check::passing_test_packages(cas_root, proof_repo, head);
    for_tested_packages(proof_repo, task, changed_paths, head, &tested)
}

/// Derive scope from the same receipt view used for this close's audit or
/// changed-crate gate, without rereading or reclassifying its proof inputs.
pub(super) fn for_tested_packages(
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
    tested: &[String],
) -> Option<(Task, String)> {
    if !task.risk.contains(&TaskRisk::BlastRadius) {
        return None;
    }
    let scope = derive(proof_repo, changed_paths, &task.proof_targets, tested);
    (!scope.is_empty()).then(|| (scope.widen(task), scope.note(head)))
}

/// The refusal a delivery gets at park, while its worker can still fix it.
///
/// Capped targeted tests prove each changed crate at this exact tip. Park
/// only reads those receipts; it never runs checks. Fast-row proof belongs
/// to CI because local fast-row receipts are worker-writable. Park also
/// requires a titled CHANGELOG `[Unreleased]` entry for product source.
pub(super) fn park_refusal(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
    target: &str,
    delivery_ranges: Option<&[(String, String)]>,
) -> Option<String> {
    let refusals = park_refusals(cas_root, proof_repo, task, changed_paths, head, target, delivery_ranges);
    (!refusals.is_empty()).then(|| super::gate_preflight::park_reply(refusals))
}

#[cfg(test)]
thread_local! {
    /// The OS park judges `cfg(target_os)` gates against, so one host can
    /// replay a park from the other (GH #2320).
    static PARK_HOST_OS: std::cell::Cell<&'static str> = const { std::cell::Cell::new(std::env::consts::OS) };
}

fn park_host_os() -> &'static str {
    #[cfg(test)]
    return PARK_HOST_OS.get();
    #[cfg(not(test))]
    std::env::consts::OS
}

/// The class selector the last request_changes recorded for this task.
fn recorded_class_audit(cas_root: &Path, task_id: &str) -> Option<String> {
    crate::mcp::tools::core::task::repair_class::recorded_class(
        crate::store::open_task_store(cas_root).ok()?.as_ref(), task_id)
}

/// Every receipt-scope refusal at this tip, not just the first. Each runner
/// config is listed once for the whole judgement (GH #2331).
pub(super) fn park_refusals(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
    target: &str,
    delivery_ranges: Option<&[(String, String)]>,
) -> Vec<String> {
    crate::factory_worker_check::javascript::with_listing_cache(||
        listed_park_refusals(cas_root, proof_repo, task, changed_paths, head, target, delivery_ranges))
}

pub(super) fn delivered_tests(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    target: &str,
    paths: &[String],
    ranges: Option<&[(String, String)]>,
) -> anyhow::Result<(Vec<stability::Identity>, Vec<Scope>)> {
    let (mut tests, mut widened) = stability::requirements(repo, head, target, paths, ranges)?;
    // GH #2362, #2380: a registered journey against a deployed host follows
    // its declared lane, so neither it nor a helper only such journeys
    // import owes a worker run here; a locally served journey stays owed.
    let journeys = widen::deployed_journey_specs(cas_root, repo, head);
    tests.retain(|test| !journeys.contains(&test.path));
    widened.retain(|scope| match scope {
        Scope::File(spec) => !journeys.contains(spec),
        Scope::Related(helper) if !journeys.is_empty() => {
            let consumers = crate::factory_worker_check::javascript::related_specs(repo, helper);
            consumers.is_empty() || consumers.iter().any(|(runner, specs)| {
                *runner != "playwright" || specs.iter().any(|spec| !journeys.contains(spec))
            })
        }
        _ => true,
    });
    Ok((tests, widened))
}

pub(super) fn red_first_refusal(
    task: &Task, passing: &PassingTests<'_>, head: &str,
    ranges: &[(String, String)], delivered: &[stability::Identity], widened: &[Scope],
) -> Option<String> {
    if task.execution_note.as_deref() != Some("test-first") || (delivered.is_empty() && !widened.is_empty()) { return None; }
    let start_base = task.deliverables.factory_branch_start.as_ref().map(|start| start.base_sha.as_str());
    if passing.has_red_first_evidence(&task.id, start_base, ranges, delivered) { return None; }
    let base = start_base.unwrap_or("the recorded task start base");
    let short = &head[..head.len().min(12)];
    // gate[G-170 park as-of]: Test-first delivery requires a capped failing run before the fix
    Some(crate::gates::tag("G-170", format!(
        "RED-FIRST EVIDENCE REQUIRED: Task {} declares execution_note=test-first. \
         In its assigned checkout, record the delivered regression failing at {base} or at a committed red checkpoint \
         in this delivery before {short}. Run `cargo nextest run -p <crate> --lib -E 'test(<name>)'` \
         (or `--test <harness>` instead of `--lib`) or a capped JS test, for example \
         `cas factory worker-check --cas-root <cas-root> -- node --test <file>`. \
         A capped compile failure at {base} also qualifies: run `cargo check -p <crate> --tests`. \
         Return to the fixed tip and record passing proof for the same regression before retrying park.",
        task.id,
    )))
}

fn listed_park_refusals(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
    target: &str,
    delivery_ranges: Option<&[(String, String)]>,
) -> Vec<String> {
    let short = &head[..head.len().min(12)];
    let mut refusals: Vec<_> =
        changelog_refusal(proof_repo, task, changed_paths, head, target, delivery_ranges).into_iter().collect();
    refusals.extend(super::doc_fragment_gate::park_refusals(cas_root, proof_repo, task, changed_paths, head));
    // Judge the immutable delivery delta as well as caller paths: an earlier
    // push can introduce a retained checkpoint absent from a later path list.
    // A project-owned file already present at the base remains inherited.
    let git = |args: &[&str]| std::process::Command::new("git").args(args)
        .current_dir(proof_repo).output().ok().filter(|output| output.status.success());
    let handoff_changed = changed_paths.iter().any(|path| path == "HANDOFF.md") || {
        let ranges = delivery_ranges.map(<[_]>::to_vec).unwrap_or_else(|| {
            git(&["merge-base", head, target]).map(|output|
                vec![(String::from_utf8_lossy(&output.stdout).trim().to_string(), head.to_string())])
                .unwrap_or_default()
        });
        ranges.iter().any(|(base, tip)| git(&["diff", "--name-only", base, tip, "--", "HANDOFF.md"])
            .is_some_and(|output| !output.stdout.is_empty()))
    };
    if handoff_changed
        && std::process::Command::new("git")
        .args(["ls-tree", "--name-only", head, "--", "HANDOFF.md"])
        .current_dir(proof_repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty())
    {
        refusals.push(format!(
            "ROOT_HANDOFF_COMMITTED: task {} delivers repo-root HANDOFF.md at {short}. Move checkpoint notes to this task's artifacts directory (HANDOFF.md) or task notes, remove the root file from the delivery, commit and push the correction, then retry park.",
            task.id,
        ));
    }
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    // A receipt minted on the machine that can build a platform-gated test
    // travels on the task row (cas-289b), so park reads it too.
    let passing = PassingTests::new(cas_root, proof_repo, head).with_target(target)
        .with_synced(&task.deliverables.test_receipts);
    if proof_repo.join(".git").exists() {
        let started = std::time::Instant::now();
        let requirements = delivered_tests(cas_root, proof_repo, head, target, changed_paths, delivery_ranges);
        tracing::info!(task_id = %task.id, gate = "G-61", phase = "changed_test_discovery", head,
            elapsed_ms = started.elapsed().as_millis() as u64, "Close gate phase");
        let (tests, widened) = match requirements {
            Ok((tests, widened)) => {
                refusals.extend(red_first_refusal(task, &passing, head, delivery_ranges.unwrap_or_default(), &tests, &widened));
                (tests, widened)
            }
            Err(error) => {
                // A declared Rust test absent from every Cargo target is
                // proven missing, rather than an uncertain classification.
                refusals.push(crate::gates::tag("G-61", format!("CHANGED_TEST_DISCOVERY_FAILED: {error:#}")));
                Default::default()
            }
        };
        // GH #2355: a deleted file owes nothing and is no package input.
        let present: Vec<String> = changed_paths.iter().filter(|path| std::process::Command::new("git")
            .args(["cat-file", "-e", &format!("{head}:{path}")]).current_dir(proof_repo)
            .output().is_ok_and(|output| output.status.success())).cloned().collect();
        let mut missing = stability::missing(&passing, &tests);
        // GH #2369: a selection that could not run a test ran its package.
        missing.retain(|test| !widen::package_proves(&passing, &test.path));
        // GH #2320: a test gated to another OS selects nothing here, so this
        // host can never run it. Lane CI's Rust test job, green at this tip,
        // proves the lane OS's tests; any other OS needs five passes recorded
        // on that OS.
        let gates = stability::rust::platform_gates(proof_repo, head, &missing).unwrap_or_default();
        let (foreign, missing): (Vec<_>, Vec<_>) = missing.into_iter()
            .partition(|test| gates.get(test).is_some_and(|os| !os.contains(park_host_os())));
        let lane = foreign.iter().any(|test| gates[test].contains(stability::LANE_CI_OS))
            .then(|| crate::mcp::tools::core::workflow::delivery_tip_rust_tests(head, proof_repo));
        let foreign = foreign.into_iter()
            .filter(|test| !(lane.is_some_and(|(_, tested)| tested) && gates[test].contains(stability::LANE_CI_OS)))
            .collect::<Vec<_>>();
        if !foreign.is_empty() {
            refusals.push(crate::gates::tag("G-61", format!(
                "PLATFORM_TEST_PROOF_REQUIRED: task {} changes tests that this {host} host cannot build: {}. Do not run them here; the capped runner selects 0 of them. A test that builds on {lane_os} is proven when lane CI's Rust test job passes at {short} (lane CI now: {state}); a test for any other OS needs five consecutive capped passes recorded on that OS at {short}. Then retry park.",
                task.id,
                foreign.iter().map(|test| format!("{}::{} (target_os {})", test.path, test.name,
                    gates[test].iter().cloned().collect::<Vec<_>>().join(" or "))).collect::<Vec<_>>().join(", "),
                host = park_host_os(), lane_os = stability::LANE_CI_OS, state = lane.map_or("not needed", |(conclusion, tested)| if conclusion == "success" && !tested { "green without a Rust test job" } else { conclusion }),
            )));
        }
        if !missing.is_empty() {
            // GH #2288: name why an earlier five-pass proof did not carry.
            let voided = stability::invalidated(&passing, &missing).into_iter().map(|(tested, inputs)| {
                let tested = &tested[..tested.len().min(12)];
                match inputs {
                    Some(inputs) => format!(" Proof at {tested} does not carry: {short} changes its inputs {}.", inputs.join(", ")),
                    None => format!(" Proof at {tested} does not carry: not an ancestor of {short}."),
                }
            }).collect::<String>();
            refusals.push(crate::gates::tag("G-61", format!(
                "CHANGED_TEST_STABILITY_REQUIRED: task {} needs five consecutive capped passes at {short} for: {}.{voided} Fix or delete a failing test; never retry-wrap it. Run only these changed tests through the capped runner: {}. Then retry park.",
                task.id, missing.iter().map(|test| format!("{}::{}", test.path, test.name)).collect::<Vec<_>>().join(", "),
                stability::rerun_commands(cas_root, proof_repo, head, &missing).join("; "),
            )));
        }
        let covering = widen::covering(proof_repo, head, &present, &widened);
        let missing = widen::missing(&passing, &widened, &covering);
        if !missing.is_empty() {
            let notes = widen::coverage_notes(&missing, &covering);
            // Classification is uncertain, but the broader capped receipt is
            // proven missing. Park reads proof; it never starts a test process.
            refusals.push(crate::gates::tag("G-61", format!(
                "FOCUSED PROOF REQUIRED: task {} widened uncertain test selection to {missing:?}; one passing capped run is missing at {short}.{notes} Run: {}. Identified changed tests still require five passes.",
                task.id, widen::commands(&passing, &present, &missing, &covering).join("; ")
            )));
        }
    }
    // cas-bc25: a repair verdict that named failing tests also named their
    // class; park needs a passing capped run of all of it at this tip.
    if let Some(class) = recorded_class_audit(cas_root, &task.id)
        && !crate::mcp::tools::core::task::repair_class::receipts_cover_class(&passing.filters(), &class)
    {
        // cas-8a5c: name the Cargo targets that register the class; an
        // integration module is not in --lib.
        let targets = crate::factory_worker_check::stability::rust::class_targets(proof_repo, head, changed_paths, &class)
            .unwrap_or_default();
        let commands = if targets.is_empty() {
            format!("`cargo nextest run -p <crate> <--lib | --test <harness>> -E '{class}'`, choosing the Cargo target that registers it")
        } else {
            targets.iter().map(|(package, harness)| format!("`cargo nextest run -p {package} {} -E '{class}'`",
                harness.as_ref().map_or_else(|| "--lib".to_owned(), |harness| format!("--test {harness}"))))
                .collect::<Vec<_>>().join("; ")
        };
        refusals.push(crate::gates::tag("G-61", format!(
            "CLASS_AUDIT_REQUIRED: task {} was sent back with class audit `{class}`, and no passing capped receipt at {short} runs all of it. Run it through the capped runner ({commands}), fix what fails, then retry park.",
            task.id,
        )));
    }
    let tested = passing.packages();
    let untested = untested_changed_crates(proof_repo, changed_paths, &tested);
    // The module check below reads these receipts, so a crate that still owes
    // one cannot be judged yet; running its tests may already cover the module.
    if !untested.is_empty() {
        refusals.push(crate::gates::tag("G-61", format!(
            "⚠️ FOCUSED PROOF REQUIRED (cas-eac2): task {id} changes Rust in crates that have no passing worker test receipt at {short}: {crates}. Lane CI no longer runs broad Rust tests for factory pushes, so the targeted fallback is yours.\n\nNext: At {short}, run a capped targeted test for the changed behaviour in each crate (`cargo nextest run -p <crate> --lib -E 'test(<name>)'`, or `--test <harness>`), then retry `{caller}task action=close id={id}`. A receipt from another crate does not count.",
            id = task.id,
            crates = untested.join(", "),
        )));
        return refusals;
    }
    // cas-d6eb: a blast-radius module neither declared nor covered by a
    // receipt, while proof_targets can still be widened (parking locks them).
    if !task.risk.contains(&TaskRisk::BlastRadius) {
        return refusals;
    }
    let judged = for_tested_packages(proof_repo, task, changed_paths, head, &tested);
    let targets = judged.as_ref().map_or(&task.proof_targets, |(judged, _)| &judged.proof_targets);
    let uncovered = super::uncovered_blast_radius_modules(proof_repo, changed_paths, targets);
    if uncovered.is_empty() {
        return refusals;
    }
    let mut widened = cas_types::normalize_proof_targets(&task.proof_targets);
    for module in &uncovered {
        if !widened.contains(module) {
            widened.push(module.clone());
        }
    }
    refusals.push(crate::gates::tag("G-61", format!(
        "⚠️ PROOF SCOPE INCOMPLETE (cas-d6eb): task {id} declares risk=blast-radius, and its delivery at {short} reaches source modules that neither its proof_targets nor a passing worker test receipt at that commit covers: {modules}. Parking locks the proof scope, so fix it now: run a capped targeted test in each module's crate at {short} (`cargo nextest run -p <crate> --lib -E 'test(<name>)'`), or widen the declaration with `{caller}task action=update id={id} proof_targets=\"{targets}\"`. Then retry `{caller}task action=close id={id}`.",
        id = task.id,
        modules = uncovered.join(", "),
        targets = widened.join(","),
    )));
    refusals
}


pub(super) fn product_source_path(path: &str) -> bool {
    if path.starts_with("cas-cli/src/builtins/skills/") {
        return true;
    }
    if (path.starts_with("cas-cli/src/") || (path.starts_with("crates/") && path.contains("/src/")))
        && path.ends_with(".rs")
    {
        return true;
    }
    if path.starts_with("hub-web/src/") {
        return true;
    }
    if !path.starts_with("scripts/") || path.split('/').any(|part| matches!(part, "tests" | "fixtures")) {
        return false;
    }
    let file = path.rsplit('/').next().unwrap_or_default();
    if file.starts_with("test-") || file.contains(".test.") || file.contains("_test.") {
        return false;
    }
    [".sh", ".py", ".js", ".mjs", ".ts", ".ps1"]
        .iter()
        .any(|extension| file.ends_with(extension))
}

fn changelog_refusal(
    repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
    target: &str,
    delivery_ranges: Option<&[(String, String)]>,
) -> Option<String> {
    if !changed_paths.iter().any(|path| product_source_path(path))
        || has_no_changelog_decision(&task.notes)
        || !has_unreleased_section_at_head(repo, head)
    {
        return None;
    }
    let fragments = has_changelog_fragments_at(repo, head);
    let changelog_changed = changed_paths
        .iter()
        .any(|path| if fragments { is_changelog_fragment(path) } else { path == "CHANGELOG.md" });
    // gate[G-62 park as-of]: CHANGELOG [Unreleased] entry or fragment required
    if changelog_changed && has_titled_unreleased_delta(repo, head, target, delivery_ranges, &task.id) {
        return None;
    }
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    let entry = if fragments {
        format!(
            "Add a fragment file `{CHANGELOG_FRAGMENT_DIR}/{}.md` holding a `### Added — <area>` or `### Fixed — <area>` heading and a per-delivery line such as `- **Title** — What changed.`; do not edit `CHANGELOG.md` itself, because concurrent pull requests that insert at the top of `## [Unreleased]` conflict in GitHub's merge queue. The release cut folds fragments into the version section.",
            task.id,
        )
    } else {
        "Add a per-delivery line such as `- **Title** — What changed.` under a `### Added — <area>` or `### Fixed — <area>` heading.".to_string()
    };
    let inspected_ranges = delivery_ranges
        .map(|ranges| format!("{ranges:?}"))
        .unwrap_or_else(|| format!("first-parent history relative to {target}"));
    Some(crate::gates::tag("G-62", format!(
        "⚠️ CHANGELOG [Unreleased] REQUIRED (cas-a945): task {} changes shipped product or developer source but has no new or extended titled entry under `CHANGELOG.md`'s `## [Unreleased]` section. Inspected commit {head}; target {target}; delivery ranges {inspected_ranges}.\n\nNext: {entry} Then retry `{caller}task action=close id={}`. For test-only, docs-only or internal QA work, record `{caller}task action=notes id={} note_type=decision notes=\"no changelog: <reason>\"` with a specific reason.",
        task.id, task.id, task.id,
    )))
}

/// cas-f2be: per-delivery changelog fragments. GitHub's mergeability check
/// and merge queue ignore the local `cas-changelog` merge driver, so two pull
/// requests that each insert a section at the top of `## [Unreleased]`
/// conflict there and the later one is dequeued. A project that keeps a
/// `changelog.d/` directory takes each delivery's Unreleased entry as its own
/// file instead; `scripts/release_changelog.py promote` folds them in.
pub(super) const CHANGELOG_FRAGMENT_DIR: &str = "changelog.d";

fn is_changelog_fragment(path: &str) -> bool {
    path.strip_prefix("changelog.d/")
        .is_some_and(|name| name.ends_with(".md") && !name.contains('/') && name != "README.md")
}

/// Whether `commit` tracks a `changelog.d/` directory, which switches the
/// changelog gate from `CHANGELOG.md` edits to fragment files.
pub(super) fn has_changelog_fragments_at(repo: &Path, commit: &str) -> bool {
    has_fragment_dir_at(repo, commit, CHANGELOG_FRAGMENT_DIR)
}

/// Whether `dir` holds any tracked file at `commit`.
pub(super) fn has_fragment_dir_at(repo: &Path, commit: &str, dir: &str) -> bool {
    std::process::Command::new("git")
        .args(["ls-tree", "--name-only", commit, "--", &format!("{dir}/")])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| !output.stdout.is_empty())
}

fn has_no_changelog_decision(notes: &str) -> bool {
    has_reasoned_decision(notes, "no changelog:")
}

/// A timestamped `✅ DECISION <prefix> <reason>` note with a real reason.
pub(super) fn has_reasoned_decision(notes: &str, prefix: &str) -> bool {
    notes.lines().any(|line| {
        let Some((stamp, body)) = line
            .strip_prefix('[')
            .and_then(|line| line.split_once("] ✅ DECISION "))
        else {
            return false;
        };
        if chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").is_err() {
            return false;
        }
        body.strip_prefix(prefix).is_some_and(|reason| {
            let reason = reason.trim();
            !reason.is_empty() && !is_changelog_template_value(reason)
        })
    })
}

pub(super) fn has_unreleased_section_at_head(repo: &Path, head: &str) -> bool {
    let reference = format!("{head}:CHANGELOG.md");
    std::process::Command::new("git")
        .args(["show", &reference])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "## [Unreleased]")
        })
}

/// Whether one of this task's delivery segments adds or changes a titled
/// Unreleased entry that survives at HEAD; in a project with `changelog.d/`
/// only fragment files count (cas-f2be). `delivery_ranges` are the close's
/// task-attributed (base, tip) segments, which keep a same-task predecessor
/// merge and drop target-sync and other tasks' merges (cas-2a65). Without
/// them, first-parent commits since the merge-base count. A merge can credit
/// only this task's fragment newly authored in its result, absent from every
/// parent, so entries imported from another branch cannot count. A fragment
/// already changed by this task's segments retains that provenance when a
/// target-sync merge revises it; validate its current entry against that base.
fn has_titled_unreleased_delta(
    repo: &Path,
    head: &str,
    target: &str,
    delivery_ranges: Option<&[(String, String)]>,
    task_id: &str,
) -> bool {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let fragments = has_changelog_fragments_at(repo, head);
    // The Unreleased entries at `commit` in `paths`: CHANGELOG.md's section,
    // or each fragment parsed as the body of its own Unreleased section.
    let entries_at = |commit: &str, paths: &[String]| {
        let mut entries = BTreeMap::new();
        for path in paths {
            let Some(text) = git(&["show", &format!("{commit}:{path}")]) else {
                continue;
            };
            let text = if fragments { format!("## [Unreleased]\n{text}") } else { text };
            entries.extend(titled_unreleased_entries(&text)?);
        }
        Some(entries)
    };
    let changelog = vec!["CHANGELOG.md".to_string()];
    let ranges: Vec<(Vec<String>, String)> = match delivery_ranges {
        Some(ranges) => ranges.iter().map(|(base, tip)| (vec![base.clone()], tip.clone())).collect(),
        None => {
            let Some(base) = git(&["merge-base", head, target]) else {
                return false;
            };
            let range = format!("{}..{head}", base.trim());
            let Some(commits) = git(&["rev-list", "--first-parent", "--parents", &range]) else {
                return false;
            };
            commits
                .lines()
                .filter_map(|line| match line.split_whitespace().collect::<Vec<_>>()[..] {
                    [commit, ref parents @ ..] if !parents.is_empty() => {
                        Some((parents.iter().map(|parent| parent.to_string()).collect(), commit.to_string()))
                    }
                    _ => None,
                })
                .collect()
        }
    };
    ranges.iter().any(|(parents, tip)| {
        let base = &parents[0];
        let paths = if parents.len() > 1 {
            let path = format!("{CHANGELOG_FRAGMENT_DIR}/{task_id}.md");
            if !fragments || parents.iter().any(|parent| {
                git(&["ls-tree", "--name-only", parent, "--", &path])
                    .is_none_or(|tracked| !tracked.trim().is_empty())
            }) {
                return false;
            }
            vec![path]
        } else if fragments {
            let Some(changed) = git(&["diff", "--no-renames", "--name-only", base, tip, "--", CHANGELOG_FRAGMENT_DIR])
            else {
                return false;
            };
            changed.lines().filter(|path| is_changelog_fragment(path)).map(str::to_string).collect()
        } else {
            changelog.clone()
        };
        // A returned delivery's target-sync merge is deliberately outside
        // its attributed segments. Credit the tip's own fragment when a
        // segment changed that file, even if the merge replaced its entry.
        // Imported fragments without that provenance cannot use this path.
        let own_fragment = format!("{CHANGELOG_FRAGMENT_DIR}/{task_id}.md");
        if fragments && paths.contains(&own_fragment) {
            let owned = [own_fragment];
            if let (Some(before), Some(final_entries)) = (entries_at(base, &owned), entries_at(head, &owned))
                && final_entries.iter().any(|(key, entry)| before.get(key) != Some(entry))
            {
                return true;
            }
        }
        let (Some(after), Some(final_entries)) = (entries_at(tip, &paths), entries_at(head, &paths)) else {
            return false;
        };
        let before = entries_at(base, &paths).unwrap_or_default();
        after.iter().any(|(key, entry)| {
            before.get(key) != Some(entry) && final_entries.get(key) == Some(entry)
        })
    })
}

fn titled_unreleased_entries(text: &str) -> Option<BTreeMap<(String, String), String>> {
    let lines: Vec<_> = text.lines().collect();
    let start = lines.iter().position(|line| *line == "## [Unreleased]")? + 1;
    let end = lines[start..]
        .iter()
        .position(|line| line.starts_with("## ["))
        .map_or(lines.len(), |offset| start + offset);
    let mut entries = BTreeMap::new();
    let mut area = None::<String>;
    let mut index = start;
    while index < end {
        let line = lines[index];
        if let Some(heading) = line.strip_prefix("### ") {
            area = changelog_area(heading);
            index += 1;
            continue;
        }
        if let Some(bullet) = line.strip_prefix("- **")
            && let Some((title, description)) = bullet.split_once("** — ")
            && !title.trim().is_empty()
            && !description.trim().is_empty()
            && !is_changelog_template_value(title)
            && !is_changelog_template_value(description)
        {
            let mut block = vec![line];
            index += 1;
            while index < end
                && !lines[index].trim().is_empty()
                && !lines[index].starts_with("- ")
                && !lines[index].starts_with("### ")
            {
                block.push(lines[index]);
                index += 1;
            }
            if let Some(area) = area.as_ref() {
                entries.insert((area.clone(), title.trim().to_string()), block.join("\n"));
            }
            continue;
        }
        index += 1;
    }
    Some(entries)
}

fn changelog_area(heading: &str) -> Option<String> {
    let (kind, area) = heading.split_once(" — ")?;
    let area = area.trim();
    if ![
        "Added",
        "Changed",
        "Deprecated",
        "Removed",
        "Fixed",
        "Security",
    ]
    .contains(&kind)
        || area.is_empty()
        || is_changelog_template_value(area)
    {
        return None;
    }
    Some(format!("{kind} — {area}"))
}

fn is_changelog_template_value(value: &str) -> bool {
    let value = value.trim();
    value.eq_ignore_ascii_case("title")
        || value.eq_ignore_ascii_case("what changed.")
        || (value.starts_with('<') && value.strip_suffix('.').unwrap_or(value).ends_with('>'))
}

/// cas-d6eb: the tested packages that cover every target a proof-scope
/// widening adds, when each target is crate-qualified (`<crate>::<path>`) and
/// its crate has a passing worker test receipt at `head`. Such a widening only
/// names what the close gate already accepts at that head ([`for_tested_packages`]), so
/// it need not invalidate the reviewed proof cycle. `None` otherwise.
pub(super) fn receipts_cover_targets(
    cas_root: &Path,
    repo: &Path,
    head: &str,
    added: &[String],
) -> Option<Vec<String>> {
    let tested = crate::factory_worker_check::passing_test_packages(cas_root, repo, head);
    if tested.is_empty() || added.is_empty() {
        return None;
    }
    let package_dirs = workspace_package_dirs(repo);
    let mut packages = Vec::new();
    for target in added {
        let segments = super::proof_target_segments(target);
        let (first, rest) = segments.split_first()?;
        if rest.is_empty() {
            return None;
        }
        // A crate directory, or a package name the manifests map to one.
        let crate_dir = if package_dirs.values().any(|dir| dir == first) {
            first.clone()
        } else {
            package_dirs.get(first)?.clone()
        };
        let module = SourceModule { crate_dir, segments: rest.to_vec(), test_parent: None, ambiguous_test_parents: Vec::new() };
        let package = tested
            .iter()
            .find(|package| package_covers(package, &module, &package_dirs))?;
        if !packages.contains(package) {
            packages.push(package.clone());
        }
    }
    Some(packages)
}

/// Delivered source modules the declared `proof_targets` miss but a passing
/// targeted test in the same crate covers. `tested_packages` are the Cargo
/// package names of the receipts at the delivered head.
pub(super) fn derive(
    repo: &Path,
    changed_paths: &[String],
    proof_targets: &[String],
    tested_packages: &[String],
) -> ReceiptScope {
    if tested_packages.is_empty() {
        return ReceiptScope::default();
    }
    let declared = cas_types::normalize_proof_targets(proof_targets);
    let package_dirs = workspace_package_dirs(repo);
    let mut scope = ReceiptScope::default();
    for module in changed_source_modules(repo, changed_paths) {
        if declared
            .iter()
            .any(|target| proof_target_matches_module(target, &module))
        {
            continue;
        }
        let Some(package) = tested_packages
            .iter()
            .find(|package| package_covers(package, &module, &package_dirs))
        else {
            continue;
        };
        let qualified = format!("{}::{}", module.crate_dir, module.display());
        if !scope.modules.contains(&qualified) {
            scope.modules.push(qualified);
        }
        if !scope.packages.contains(package) {
            scope.packages.push(package.clone());
        }
    }
    scope
}

/// Whether a test receipt for `package` exercised the crate holding `module`.
/// The workspace manifests are authoritative (`crates/cas-mcp-proxy` is the
/// package `code-mode-mcp`); a package they do not name falls back to the
/// directory aliases a proof target may use (`cas-cli` is package `cas`).
fn package_covers(
    package: &str,
    module: &SourceModule,
    package_dirs: &BTreeMap<String, String>,
) -> bool {
    let package = package.to_ascii_lowercase();
    match package_dirs.get(&package) {
        Some(dir) => *dir == module.crate_dir,
        None => module
            .crate_aliases()
            .iter()
            .any(|alias| *alias == package || *alias == package.replace('-', "_")),
    }
}

/// cas-eac2 (D1): workspace packages whose Rust sources or tests the delivery
/// changes and that no passing test receipt in `tested` covers. Matching is
/// [`package_covers`], the one [`derive`] uses, so a receipt in an unrelated
/// crate never satisfies a changed one. Paths outside a workspace crate, such
/// as a repository without Cargo manifests, are not Rust changes.
fn untested_changed_crates(repo: &Path, changed_paths: &[String], tested: &[String]) -> Vec<String> {
    let package_dirs = workspace_package_dirs(repo);
    let tests = changed_paths.iter().filter_map(|path| {
        let (krate, _) = path.split_once("/tests/").filter(|_| path.ends_with(".rs"))?;
        let crate_dir = krate.rsplit('/').next()?.to_ascii_lowercase();
        Some(SourceModule { crate_dir, segments: Vec::new(), test_parent: None, ambiguous_test_parents: Vec::new() })
    });
    let mut untested = Vec::new();
    for module in changed_source_modules(repo, changed_paths).into_iter().chain(tests) {
        let Some((package, _)) = package_dirs.iter().find(|(_, dir)| **dir == module.crate_dir) else {
            continue;
        };
        if !untested.contains(package)
            && !tested.iter().any(|tested| package_covers(tested, &module, &package_dirs))
        {
            untested.push(package.clone());
        }
    }
    untested.sort();
    untested
}

/// The root and member manifests of the workspace at `repo`.
fn workspace_manifests(repo: &Path) -> Vec<std::path::PathBuf> {
    let Some(root) = read_manifest(&repo.join("Cargo.toml")) else {
        return Vec::new();
    };
    let mut manifests = vec![repo.join("Cargo.toml")];
    if let Some(members) = root
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
    {
        for member in members.iter().filter_map(toml::Value::as_str) {
            let pattern = repo.join(member).join("Cargo.toml");
            if let Ok(paths) = glob::glob(&pattern.to_string_lossy()) {
                manifests.extend(paths.flatten());
            }
        }
    }
    manifests
}

/// Map each workspace package name to its crate directory's name, as
/// [`changed_source_modules`] records it (lowercase basename).
pub(super) fn workspace_package_dirs(repo: &Path) -> BTreeMap<String, String> {
    let mut dirs = BTreeMap::new();
    for path in workspace_manifests(repo) {
        let Some(name) = read_manifest(&path).and_then(|manifest| {
            manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .map(str::to_ascii_lowercase)
        }) else {
            continue;
        };
        let Some(dir) = path
            .parent()
            .and_then(Path::file_name)
            .map(|dir| dir.to_string_lossy().to_ascii_lowercase())
        else {
            continue;
        };
        dirs.entry(name).or_insert(dir);
    }
    dirs
}

fn read_manifest(path: &Path) -> Option<toml::Value> {
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_root_handoff_is_not_a_delivery_cas_6583() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(repo)
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.test"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("HANDOFF.md"), "project-owned document\n").unwrap();
        git(&["add", "."]); git(&["commit", "-qm", "baseline"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.join("guide.md"), "delivery\n").unwrap();
        git(&["add", "."]); git(&["commit", "-qm", "delivery"]);
        let task = Task::new("cas-fixture".into(), "fixture".into());
        let root = crate::store::init_cas_dir(repo).unwrap();
        for introduced in [false, true] {
            if introduced {
                std::fs::write(repo.join("HANDOFF.md"), "worker checkpoint\n").unwrap();
                git(&["add", "HANDOFF.md"]); git(&["commit", "-qm", "checkpoint"]);
            }
            let head = git(&["rev-parse", "HEAD"]);
            let changed = if introduced { vec!["HANDOFF.md".into(), "guide.md".into()] } else { vec!["guide.md".into()] };
            let refusals = park_refusals(&root, repo, &task, &changed, &head, &base, None);
            assert_eq!(refusals.iter().any(|reason| reason.split_once(':').is_some_and(|(code, _)| code == "ROOT_HANDOFF_COMMITTED")), introduced, "{refusals:?}");
        }
    }

    /// cas-eac2 (D1): every changed workspace crate, by source or test file,
    /// needs its own receipt. A receipt in a dependent or unrelated crate never
    /// stands in for it (the cas-6c13 weakness), and a path outside the
    /// workspace's crates is not a Rust change.
    #[test]
    fn each_changed_crate_needs_its_own_receipt_cas_eac2() {
        let repo = workspace();
        let changed = vec![
            "crates/cas-store/src/prompt_queue_store.rs".to_string(),
            "cas-cli/tests/factory_test.rs".to_string(),
            "web/src/app.rs".to_string(),
            "docs/notes.md".to_string(),
        ];
        let untested = |tested: &[&str]| {
            let tested: Vec<String> = tested.iter().map(|p| p.to_string()).collect();
            untested_changed_crates(repo.path(), &changed, &tested)
        };
        assert_eq!(untested(&[]), ["cas", "cas-store"]);
        assert_eq!(untested(&["code-mode-mcp", "cas-mcp"]), ["cas", "cas-store"]);
        assert_eq!(untested(&["cas"]), ["cas-store"]);
        assert!(untested(&["cas", "cas-store"]).is_empty());
    }

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"cas-cli\", \"crates/*\"]\n",
        )
        .unwrap();
        for (dir, package) in [
            ("cas-cli", "cas"),
            ("crates/cas-store", "cas-store"),
            ("crates/cas-mcp-proxy", "code-mode-mcp"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(
                root.join(dir).join("Cargo.toml"),
                format!("[package]\nname = \"{package}\"\n"),
            )
            .unwrap();
        }
        dir
    }

    fn blast_radius_task(targets: &[&str]) -> Task {
        let mut task = Task::new("cas-b38a-fixture".into(), "delivery".into());
        task.risk = vec![TaskRisk::BlastRadius];
        task.proof_targets = targets.iter().map(|target| target.to_string()).collect();
        task
    }

    fn gate(task: &Task, changed: &[String], repo: &Path) -> Result<(), String> {
        let mut cache = super::super::ScopedProofTargetCache::default();
        super::super::validate_risk_close_proofs_with_base_and_target_and_cache(
            task,
            changed,
            repo,
            repo,
            None,
            None,
            super::super::BuildProofs::DeferredToAssembly,
            &mut cache,
        )
    }

    fn delivered() -> Vec<String> {
        [
            "cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs",
            "cas-cli/src/factory_worker_check.rs",
            "crates/cas-store/src/prompt_queue_store/operator_cloud.rs",
            "docs/notes.md",
        ]
        .map(String::from)
        .to_vec()
    }

    /// A worker that ran targeted tests in every crate it touched closes
    /// without a supervisor widening its declared proof targets.
    #[test]
    fn receipts_in_every_touched_crate_cover_the_delivery_cas_b38a() {
        let repo = workspace();
        let task = blast_radius_task(&["close_ops"]);
        let changed = delivered();
        let refused = gate(&task, &changed, repo.path()).unwrap_err();
        assert!(refused.contains("factory_worker_check"), "{refused}");
        assert!(
            refused.contains("prompt_queue_store::operator_cloud"),
            "{refused}"
        );

        let tested = vec!["cas".to_string(), "cas-store".to_string()];
        let scope = derive(repo.path(), &changed, &task.proof_targets, &tested);
        assert_eq!(
            scope.modules,
            [
                "cas-cli::factory_worker_check",
                "cas-store::prompt_queue_store::operator_cloud",
            ]
        );
        assert_eq!(scope.packages, ["cas", "cas-store"]);
        gate(&scope.widen(&task), &changed, repo.path())
            .expect("receipts in both crates cover every delivered module");
        // The stored task keeps its declared targets.
        assert_eq!(task.proof_targets, ["close_ops"]);
        let note = scope.note("abc123");
        assert!(
            note.contains("cas-store::prompt_queue_store::operator_cloud"),
            "{note}"
        );
        assert!(note.contains("at abc123 for cas, cas-store"), "{note}");
    }

    /// A delivered crate with no test run at all is still refused, and the
    /// refusal names exactly its modules.
    #[test]
    fn a_touched_crate_without_a_test_receipt_is_still_refused_cas_b38a() {
        let repo = workspace();
        let task = blast_radius_task(&["close_ops"]);
        let changed = delivered();
        let scope = derive(
            repo.path(),
            &changed,
            &task.proof_targets,
            &["cas".to_string()],
        );
        assert_eq!(scope.modules, ["cas-cli::factory_worker_check"]);
        let refused = gate(&scope.widen(&task), &changed, repo.path()).unwrap_err();
        assert!(
            refused.contains("uncovered source modules: prompt_queue_store::operator_cloud."),
            "{refused}"
        );
        // The tested crate's module is no longer listed as uncovered.
        assert!(
            !refused.contains("uncovered source modules: factory_worker_check"),
            "{refused}"
        );
        assert!(refused.contains("run a capped targeted test"), "{refused}");

        // No receipts derive nothing.
        assert!(derive(repo.path(), &changed, &task.proof_targets, &[]).is_empty());
    }

    /// A receipt counts for the crate its package actually is: the manifest
    /// maps `code-mode-mcp` to `crates/cas-mcp-proxy`, and a receipt for one
    /// crate never covers a sibling with a similar name.
    #[test]
    fn receipts_follow_the_workspace_manifest_package_names_cas_b38a() {
        let repo = workspace();
        let changed = vec!["crates/cas-mcp-proxy/src/lib.rs".to_string()];
        let scope = derive(repo.path(), &changed, &[], &["code-mode-mcp".to_string()]);
        assert_eq!(scope.modules, ["cas-mcp-proxy::lib"]);
        assert!(derive(repo.path(), &changed, &[], &["cas-mcp".to_string()]).is_empty());
        assert!(derive(repo.path(), &changed, &[], &["cas-store".to_string()]).is_empty());
        // Outside a readable workspace the directory aliases still apply.
        let bare = tempfile::tempdir().unwrap();
        let cli = vec!["cas-cli/src/update.rs".to_string()];
        assert_eq!(
            derive(bare.path(), &cli, &[], &["cas".to_string()]).modules,
            ["cas-cli::update"]
        );
    }
    /// The close reads the capped runner's receipts for the delivered head
    /// of this worktree, and nothing else: another head, or a task that did
    /// not declare blast-radius, is left to the gate as it was.
    #[test]
    fn the_close_reads_receipts_at_the_delivered_head_cas_b38a() {
        let repo = workspace();
        let cas_root = tempfile::tempdir().unwrap();
        let head = "c".repeat(40);
        let task = blast_radius_task(&["close_ops"]);
        let changed = delivered();
        assert!(for_close(cas_root.path(), repo.path(), &task, &changed, Some(&head)).is_none());

        for package in ["cas", "cas-store"] {
            crate::factory_worker_check::record_passing_test_for_tests(
                cas_root.path(),
                repo.path(),
                &head,
                package,
                "test(worker)",
            );
        }
        let (judged, note) = for_close(cas_root.path(), repo.path(), &task, &changed, Some(&head))
            .expect("receipts at the delivered head derive the scope");
        gate(&judged, &changed, repo.path()).expect("the derived scope passes the gate");
        assert!(
            note.contains(&format!("at {head} for cas, cas-store")),
            "{note}"
        );

        assert!(
            for_close(
                cas_root.path(),
                repo.path(),
                &task,
                &changed,
                Some(&"d".repeat(40))
            )
            .is_none()
        );
        assert!(for_close(cas_root.path(), repo.path(), &task, &changed, None).is_none());
        let mut undeclared = task.clone();
        undeclared.risk = vec![TaskRisk::None];
        assert!(
            for_close(
                cas_root.path(),
                repo.path(),
                &undeclared,
                &changed,
                Some(&head)
            )
            .is_none()
        );
    }

    /// cas-eac2 (D1) at park: a delivery reaching a crate nobody tested is
    /// refused with the targeted fallback, and widening proof_targets no longer
    /// substitutes for the test; a receipt in that crate parks it. Outside a
    /// Cargo workspace the cas-d6eb scope refusal still names the modules.
    #[test]
    fn park_requires_a_receipt_in_every_changed_crate_cas_eac2() {
        let repo = workspace();
        let cas_root = tempfile::tempdir().unwrap();
        let head = "e".repeat(40);
        let mut task = blast_radius_task(&["close_ops"]);
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: test-only receipt-gate coverage".into();
        let changed = delivered();
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas", "test(worker)",
        );
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &head, "main", None)
            .expect("an untested crate is refused at park");
        assert!(refusal.contains("FOCUSED PROOF REQUIRED"), "{refusal}");
        assert!(refusal.contains(&format!("at {}: cas-store.", &head[..12])), "{refusal}");

        task.proof_targets.push("prompt_queue_store::operator_cloud".into());
        let mut plain = task.clone();
        plain.risk = vec![TaskRisk::None];
        for task in [&task, &plain] {
            assert!(park_refusal(cas_root.path(), repo.path(), task, &changed, &head, "main", None).is_some());
        }
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas-store", "test(worker)",
        );
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &head, "main", None).is_none());

        let bare = tempfile::tempdir().unwrap();
        let mut bare_task = blast_radius_task(&[]);
        bare_task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: test-only proof-scope fixture".into();
        let scope = park_refusal(cas_root.path(), bare.path(), &bare_task, &changed, &head, "main", None)
            .expect("undeclared modules outside a workspace");
        assert!(scope.contains("PROOF SCOPE INCOMPLETE (cas-d6eb)"), "{scope}");
    }

    /// cas-bc25: a repair whose verdict recorded a test class parks only with
    /// a passing capped receipt at the tip that runs the whole class.
    #[test]
    fn park_requires_a_receipt_covering_the_recorded_class_audit_cas_bc25() {
        let repo = workspace();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let head = "e".repeat(40);
        let mut task = blast_radius_task(&["close_ops"]);
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: test-only receipt-gate coverage".into();
        task.proof_targets.push("prompt_queue_store::operator_cloud".into());
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        let changed = delivered();
        let record = |package: &str, filter: &str| {
            crate::factory_worker_check::record_passing_test_for_tests(&root, repo.path(), &head, package, filter)
        };
        record("cas", "test(worker)");
        record("cas-store", "test(worker)");
        assert!(park_refusal(&root, repo.path(), &task, &changed, &head, "main", None).is_none());

        crate::store::open_task_store(&root).unwrap().patch_execution_state(
            &task.id, &serde_json::json!({"class_audit": "test(ui::) | test(gates::)"}),
        ).unwrap();
        let refusal = park_refusal(&root, repo.path(), &task, &changed, &head, "main", None)
            .expect("the recorded class has no covering receipt");
        assert!(refusal.contains("CLASS_AUDIT_REQUIRED"), "{refusal}");
        record("cas", "test(ui::)");
        assert!(park_refusal(&root, repo.path(), &task, &changed, &head, "main", None).is_some(),
            "half the class is not the class");
        record("cas", "test(gates::) | test(ui::) | test(extra)");
        assert!(park_refusal(&root, repo.path(), &task, &changed, &head, "main", None).is_none());

        // An exact receipt cannot cover a substring class: auth_refresh stays uncovered.
        crate::store::open_task_store(&root).unwrap().patch_execution_state(
            &task.id, &serde_json::json!({"class_audit": "test(auth)"}),
        ).unwrap();
        record("cas", "test(=auth)");
        assert!(park_refusal(&root, repo.path(), &task, &changed, &head, "main", None).is_some(),
            "test(=auth) selects one test, not the class");
        record("cas", "test(auth)");
        assert!(park_refusal(&root, repo.path(), &task, &changed, &head, "main", None).is_none());

        // cas-8a5c (cas-b36f): a class registered only in an integration
        // harness gets that harness's command, which --lib cannot run.
        let git_repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(git_repo.path())
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        for (path, body) in [
            ("Cargo.toml", "[workspace]\nmembers = [\"cas-cli\"]\n"),
            ("cas-cli/Cargo.toml", "[package]\nname = \"cas\"\nautotests = false\n[[test]]\nname = \"integration_factory\"\npath = \"tests/integration/factory.rs\"\n"),
            ("cas-cli/src/lib.rs", "pub fn f() {}\n"),
            ("cas-cli/tests/integration/factory.rs", "#[path = \"../factory_mcp_ops_test.rs\"]\nmod factory_mcp_ops_test;\n"),
            ("cas-cli/tests/factory_mcp_ops_test.rs", "#[test]\nfn test_assignment_allows() {}\n"),
        ] {
            std::fs::create_dir_all(git_repo.path().join(path).parent().unwrap()).unwrap();
            std::fs::write(git_repo.path().join(path), body).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        std::fs::write(git_repo.path().join("cas-cli/src/lib.rs"), "pub fn f() { let _ = 1; }\n").unwrap();
        git(&["commit", "-qam", "delivery"]);
        let head = git(&["rev-parse", "HEAD"]);
        let changed = vec!["cas-cli/src/lib.rs".to_string()];
        crate::factory_worker_check::record_passing_test_for_tests(&root, git_repo.path(), &head, "cas", "test(worker)");
        crate::store::open_task_store(&root).unwrap().patch_execution_state(
            &task.id, &serde_json::json!({"class_audit": "test(factory_mcp_ops_test::test_assignment_) | test(factory_mcp_ops_test::)"}),
        ).unwrap();
        let refusals = park_refusals(&root, git_repo.path(), &task, &changed, &head, "main", None);
        let class = refusals.iter().find(|refusal| refusal.contains("CLASS_AUDIT_REQUIRED")).expect("class refusal");
        assert!(class.contains("-p cas --test integration_factory -E"), "{class}");
        assert!(!class.contains("--lib"), "{class}");
    }

    #[test]
    fn park_requires_owned_test_stability_with_shared_carry_forward_cas_9e02e() {
        use crate::factory_worker_check::{PassingTests, widened, stability::{self, Identity, Proof}};
        for runner in ["nextest", "vitest", "jest", "playwright"] {
            let repo = changelog_fixture();
            let root = tempfile::tempdir().unwrap();
            let rust = runner == "nextest";
            // GH #2243: the reported Vitest proof covered one `.each` template.
            let each = matches!(runner, "vitest" | "jest");
            let path = if rust { "src/child.rs" } else { "owned.test.js" };
            if rust {
                std::fs::create_dir_all(repo.path().join("src")).unwrap();
                std::fs::write(repo.path().join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
                std::fs::write(repo.path().join("src/lib.rs"), "mod child;\n").unwrap();
            }
            let old = if rust { "#[test]\nfn untouched() {}\n" } else { "test('untouched', () => {});\n" };
            let added = if rust { "#[test]\nfn owned() {}\n" }
                else if each { "test.each([1, 2])('owned %s', () => {});\n" } else { "test('owned', () => {});\n" };
            std::fs::write(repo.path().join(path), old).unwrap();
            let base = commit_changelog_fixture(repo.path(), "base test");
            assert!(std::process::Command::new("git").args(["update-ref", "refs/heads/main", &base])
                .current_dir(repo.path()).status().unwrap().success());
            let identity = if each { Identity { path: path.into(), name: "owned %s".into(), parameterized: true, environment_gated: false } }
                else { Identity::exact(path, if rust { "child::owned" } else { "owned" }) };
            stability::record(root.path(), repo.path(), &base, Proof { runner: runner.into(),
                tests: vec![identity.clone()], consecutive_passes: 5, elapsed_ms: 0, inactive: Vec::new() }).unwrap();
            std::fs::write(repo.path().join(path), format!("{old}{added}")).unwrap();
            std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
            let head = commit_changelog_fixture(repo.path(), "add owned test");
            let mut task = Task::new("cas-9e02e".into(), "stability fixture".into());
            task.notes = "[2026-11-13 10:00] ✅ DECISION no changelog: test-only stability fixture".into();
            let paths = vec![identity.path.clone(), "hub-web/src/app.ts".into()];
            if rust { crate::factory_worker_check::record_passing_test_for_tests(root.path(), repo.path(), &head, "fixture", "test(child)"); }
            let held = park_refusal(root.path(), repo.path(), &task, &paths, &head, "main", None).unwrap();
            assert!(held.contains(&identity.path) && held.contains(&identity.name), "{runner}: {held}");
            assert!(!held.contains("untouched"));
            stability::record(root.path(), repo.path(), &head, Proof { runner: runner.into(),
                tests: vec![identity.clone()], consecutive_passes: 5, elapsed_ms: 0, inactive: Vec::new() }).unwrap();
            let packages = crate::factory_worker_check::passing_test_packages(root.path(), repo.path(), &head);
            assert_eq!(packages, if rust { vec!["fixture".to_string()] } else { vec![] });
            let admitted = park_refusal(root.path(), repo.path(), &task, &paths, &head, "main", None);
            assert!(admitted.is_none(), "{runner}: {admitted:?}");
            let scope = if rust { widened::Scope::Crate("fixture".into()) } else { widened::Scope::Package(String::new()) };
            widened::record(root.path(), repo.path(), &head, widened::Proof { runner: runner.into(),
                scope: scope.clone(), reason: "widened:unclassified".into(), configs: Default::default() }, 1).unwrap();
            // GH #2243/#2288: the docs-only child a close itself asks for,
            // replayed from cas-e705's be8ba51e0c.
            for (fragment, body) in [("changelog.d/cas-9e02e.md", "### Fixed\n- fixture\n"),
                ("docs/product-overview.d/cas-9e02e.md", "fixture\n"),
                ("docs/qa/bug-families/fixture-family.yaml", "tasks:\n  - cas-9e02e\nsummary: fixture\n")] {
                std::fs::create_dir_all(repo.path().join(fragment).parent().unwrap()).unwrap();
                std::fs::write(repo.path().join(fragment), body).unwrap();
            }
            let moved = commit_changelog_fixture(repo.path(), "move source tip");
            assert!(park_refusal(root.path(), repo.path(), &task, &paths, &moved, "main", None).is_none(),
                "{runner}: a docs-only commit must carry the tested input proof forward");
            let carried = PassingTests::new(root.path(), repo.path(), &moved).with_target("main");
            assert!(widened::missing(&carried, std::slice::from_ref(&scope), &Default::default()).is_empty(), "{runner}: widened run carries");
            // The product work is now on the target. A test-only correction
            // still owes stability, including when the test name is unchanged.
            assert!(std::process::Command::new("git").args(["update-ref", "refs/heads/main", &moved])
                .current_dir(repo.path()).status().unwrap().success());
            let edited = if rust { "#[test]\nfn owned() { assert_eq!(1, 1); }\n" }
                else if each { "test.each([1, 2])('owned %s', (row) => { expect(row).toBe(row); });\n" }
                else { "test('owned', () => { expect(1).toBe(1); });\n" };
            std::fs::write(repo.path().join(path), format!("{old}{edited}")).unwrap();
            let changed = commit_changelog_fixture(repo.path(), "edit only the owned test");
            let test_paths = vec![identity.path.clone()];
            if rust { crate::factory_worker_check::record_passing_test_for_tests(root.path(), repo.path(), &changed, "fixture", "test(child)"); }
            let held = park_refusal(root.path(), repo.path(), &task, &test_paths, &changed, "main", None).unwrap();
            assert!(crate::gates::codes_in(&held).contains(&"G-61") && held.contains(&identity.name), "{held}");
            stability::record(root.path(), repo.path(), &changed, Proof { runner: runner.into(),
                tests: vec![identity.clone()], consecutive_passes: 5, elapsed_ms: 0, inactive: Vec::new() }).unwrap();
            assert!(park_refusal(root.path(), repo.path(), &task, &test_paths, &changed, "main", None).is_none());
            std::fs::write(repo.path().join("README.md"), "after corrected test proof\n").unwrap();
            let retry = commit_changelog_fixture(repo.path(), "docs after corrected test proof");
            // A failed overlapping rerun at this docs tip may not resurrect
            // its carried ancestor proof on the next park attempt.
            stability::invalidate(root.path(), repo.path(), &retry, &[identity.clone()]).unwrap();
            let held = park_refusal(root.path(), repo.path(), &task, &test_paths, &retry, "main", None).unwrap();
            assert!(crate::gates::codes_in(&held).contains(&"G-61") && held.contains(&identity.name), "{held}");
            widened::invalidate(root.path(), repo.path(), &moved, &scope).unwrap();
            let withdrawn = PassingTests::new(root.path(), repo.path(), &moved).with_target("main");
            assert_eq!(widened::missing(&withdrawn, std::slice::from_ref(&scope), &Default::default()), [scope], "{runner}: failed rerun withdraws");
        }
    }

    /// GH #2320 (cas-2e53 at 6e0b5d82ed): park demanded five local capped
    /// passes for a changed `#[cfg(target_os = "linux")]` test on a macOS
    /// worker, where the capped runner selects 0 tests. Replayed for that
    /// host and its mirror, with the gate on the test and on its module: a
    /// test gated to the other OS owes proof on that OS, from green
    /// lane CI's Rust test job (not a lint-only green) when it builds there,
    /// else from a five-pass receipt synced from that OS, never a local run.
    #[test]
    fn changed_test_gated_to_another_os_is_proven_on_that_os_gh2320() {
        use crate::factory_worker_check::stability::{self, Identity, Proof};
        let mut env = crate::test_support::TestEnvGuard::new();
        // The incident (a Linux-only test parked from macOS), then its mirror.
        for (host, other) in [("macos", "linux"), ("linux", "macos")] {
            PARK_HOST_OS.set(host);
            let repo = changelog_fixture();
            let root = tempfile::tempdir().unwrap();
            let git = |args: &[&str]| assert!(std::process::Command::new("git").args(args)
                .current_dir(repo.path()).status().unwrap().success(), "git {args:?}");
            std::fs::create_dir_all(repo.path().join("src")).unwrap();
            std::fs::write(repo.path().join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
            std::fs::write(repo.path().join("src/lib.rs"), "mod child;\n").unwrap();
            std::fs::write(repo.path().join("src/child.rs"), "#[test]\nfn host() {}\n").unwrap();
            let base = commit_changelog_fixture(repo.path(), "base test");
            git(&["update-ref", "refs/heads/main", &base]);
            git(&["remote", "add", "origin", "https://github.com/acme/fixture.git"]);
            std::fs::write(repo.path().join("src/child.rs"), format!(
                "#[test]\nfn host() {{}}\n#[cfg(target_os = \"{other}\")]\n#[test]\nfn planted() {{ assert!(true); }}\n\
                 #[cfg(all(unix, target_os = \"{other}\"))]\nmod gated {{\n    #[test]\n    fn nested() {{ assert!(true); }}\n}}\n")).unwrap();
            let head = commit_changelog_fixture(repo.path(), "change a test gated to the other OS");
            crate::factory_worker_check::record_passing_test_for_tests(root.path(), repo.path(), &head, "fixture", "test(child)");
            let mut task = Task::new("cas-a0c0".into(), "platform-gated fixture".into());
            task.notes = "[2026-11-23 01:20] ✅ DECISION no changelog: test-only fixture".into();
            let paths = vec!["src/child.rs".to_string()];

            let gh = root.path().join("gh");
            let checks = root.path().join("checks.json");
            std::fs::write(&gh, format!(
                "#!/bin/sh\nif [ \"$1 $2\" = 'repo view' ]; then echo '{{\"nameWithOwner\":\"acme/fixture\"}}'; exit 0; fi\n\
                 [ \"$4\" = 'repos/acme/fixture/commits/{head}/check-runs' ] && cat '{}' && exit 0\nexit 97\n",
                checks.display())).unwrap();
            std::fs::set_permissions(&gh, <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755)).unwrap();
            env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
            let lane = |name: &str, status: &str, conclusion: Option<&str>| std::fs::write(&checks, serde_json::json!({"check_runs": [{
                "name": name, "status": status, "conclusion": conclusion,
                "html_url": "https://github.com/acme/fixture/actions/runs/1/job/1"}]}).to_string()).unwrap();
            let codes = |refusals: Vec<String>| refusals.iter()
                .flat_map(|refusal| ["PLATFORM_TEST_PROOF_REQUIRED", "CHANGED_TEST_STABILITY_REQUIRED"]
                    .into_iter().filter(|code| refusal.contains(code)))
                .collect::<Vec<_>>();

            let rust_job = "Scoped Validation — impact own tests";
            lane(rust_job, "in_progress", None);
            assert_eq!(codes(park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None)),
                ["PLATFORM_TEST_PROOF_REQUIRED"], "no local run is owed, and pending lane CI proves nothing");
            lane("Lint", "completed", Some("success"));
            assert_eq!(codes(park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None)),
                ["PLATFORM_TEST_PROOF_REQUIRED"], "a green lint-only lane ran no test");
            lane(rust_job, "completed", Some("success"));
            let lane_proves = other == stability::LANE_CI_OS;
            assert_eq!(park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None).is_empty(), lane_proves,
                "lane CI's Rust test job proves only a test that builds on the lane's OS");
            // The five passes ran on the machine that builds the test; only its
            // exported receipt reaches this host, on the task row.
            let producer = tempfile::tempdir().unwrap();
            stability::record(producer.path(), repo.path(), &head, Proof { runner: "nextest".into(),
                tests: ["child::planted", "child::gated::nested"].map(|name| Identity::exact("src/child.rs", name)).into(),
                consecutive_passes: 5, elapsed_ms: 0, inactive: Vec::new() }).unwrap();
            task.deliverables.test_receipts =
                crate::factory_worker_check::PassingTests::new(producer.path(), repo.path(), &head).portable();
            let refusals = park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None);
            assert!(refusals.is_empty(), "a five-pass receipt synced from the test's OS proves it: {refusals:?}");
        }
    }

    /// cas-69a0 (#2114): a queued successor keeps its older start base. After
    /// the predecessor lands and the successor rebases, park and the capped
    /// runner must name the same changed tests, and a rerun of one test must
    /// not withdraw the credit of its siblings (#2129).
    #[test]
    fn close_and_worker_check_share_the_rebased_baseline_cas_69a0() {
        use crate::factory_worker_check::{PassingTests, diff_base, javascript, stability::{self, Identity, Proof}};
        let repo = changelog_fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        let path = "owned.test.js";
        let case = |name: &str| format!("test('{name}', () => {{}});\n");
        std::fs::write(repo.path().join(path), case("untouched")).unwrap();
        let start = commit_changelog_fixture(repo.path(), "base");
        git(&["update-ref", "refs/heads/main", &start]);
        std::fs::write(repo.path().join(path), case("untouched") + &case("brand")).unwrap();
        let predecessor = commit_changelog_fixture(repo.path(), "predecessor lands");
        git(&["update-ref", "refs/remotes/origin/main", &predecessor]);
        std::fs::write(repo.path().join(path), case("untouched") + &case("brand") + &case("other")).unwrap();
        let head = commit_changelog_fixture(repo.path(), "successor, rebased");

        let paths = vec![path.to_string()];
        let close = stability::required(repo.path(), &head, "main", &paths, Some(&[(start, head.clone())])).unwrap();
        let base = diff_base(repo.path(), Some("main")).unwrap();
        let runner = javascript::changed_tests(repo.path(), &base, &head, &paths, None).unwrap();
        let other = Identity::exact(path, "other");
        assert_eq!(close, vec![other.clone()]);
        assert_eq!(runner, close);

        let root = tempfile::tempdir().unwrap();
        let sibling = Identity::exact(path, "sibling");
        let proof = |tests: Vec<Identity>| Proof { runner: "vitest".into(), tests, consecutive_passes: 5, elapsed_ms: 0, inactive: Vec::new() };
        stability::record(root.path(), repo.path(), &head, proof(vec![other.clone(), sibling.clone()])).unwrap();
        stability::invalidate(root.path(), repo.path(), &head, std::slice::from_ref(&other)).unwrap();
        let passing = PassingTests::new(root.path(), repo.path(), &head);
        assert_eq!(stability::missing(&passing, &[other, sibling]), vec![Identity::exact(path, "other")]);
    }

    /// GH #2223: a receipt that ran one test and left an environment-gated
    /// sibling inactive covers both; a failed rerun of the inactive sibling
    /// withdraws its credit and keeps the executed test's.
    #[test]
    fn failed_rerun_of_an_inactive_sibling_withdraws_its_coverage_gh2223() {
        use crate::factory_worker_check::{PassingTests, stability::{self, Identity, Proof}};
        let repo = changelog_fixture();
        let head = commit_changelog_fixture(repo.path(), "head");
        let path = "owned.test.js";
        let ran = Identity::exact(path, "executed");
        let gated = Identity { environment_gated: true, ..Identity::exact(path, "live eval") };
        let root = tempfile::tempdir().unwrap();
        stability::record(root.path(), repo.path(), &head, Proof {
            runner: "jest".into(), tests: vec![ran.clone()], consecutive_passes: 5, elapsed_ms: 0,
            inactive: vec![gated.clone()],
        }).unwrap();
        let passing = PassingTests::new(root.path(), repo.path(), &head);
        assert!(stability::missing(&passing, &[ran.clone(), gated.clone()]).is_empty());
        stability::invalidate(root.path(), repo.path(), &head, std::slice::from_ref(&gated)).unwrap();
        let passing = PassingTests::new(root.path(), repo.path(), &head);
        assert_eq!(stability::missing(&passing, &[ran, gated.clone()]), vec![gated]);
    }

    /// cas-769b at 5ddb6860: the delivery edited a test, the target rewrote
    /// it, and the sync resolved to the target side. Park walked the delivery
    /// range and still owed that test 5x while the runner, reading the net
    /// diff, refused to repeat it. Both now read the same net model.
    #[test]
    fn superseded_delivery_test_edit_owes_no_stability_cas_e751() {
        use crate::factory_worker_check::stability::{self, Identity};
        let repo = changelog_fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        std::fs::create_dir_all(repo.path().join("src")).unwrap();
        std::fs::write(repo.path().join("Cargo.toml"), "[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
        std::fs::write(repo.path().join("src/lib.rs"), "mod child;\n").unwrap();
        // Unchanged padding keeps the added test outside the conflicting hunk.
        let test = |owned: &str, body: &str|
            format!("{owned}// one\n// two\n// three\n#[test]\nfn superseded() {{ {body} }}\n");
        std::fs::write(repo.path().join("src/child.rs"), test("", "")).unwrap();
        let base = commit_changelog_fixture(repo.path(), "base test");
        git(&["update-ref", "refs/heads/main", &base]);
        std::fs::write(repo.path().join("src/child.rs"), test("#[test]\nfn owned() {}\n", "assert!(true);")).unwrap();
        let delivered = commit_changelog_fixture(repo.path(), "fix(cas-769b): edit and add tests");
        git(&["checkout", "-q", "main"]);
        std::fs::write(repo.path().join("src/child.rs"), test("", "assert_eq!(1, 1);")).unwrap();
        commit_changelog_fixture(repo.path(), "fix(cas-763f): train rewrites the test");
        git(&["checkout", "-q", "delivery"]);
        git(&["merge", "-q", "-X", "theirs", "main", "-m", "sync train"]);
        let head = commit_changelog_fixture(repo.path(), "after sync");

        let paths = vec!["src/child.rs".to_string()];
        let required = stability::required(repo.path(), &head, "main", &paths,
            Some(&[(base, delivered)])).unwrap();
        let owned = Identity::exact("src/child.rs", "child::owned");
        assert_eq!(required, vec![owned]);
        let merge_base = std::process::Command::new("git").args(["merge-base", "main", &head])
            .current_dir(repo.path()).output().unwrap();
        let runner = stability::rust::net_changed(repo.path(),
            String::from_utf8_lossy(&merge_base.stdout).trim(), &head).unwrap();
        assert_eq!(runner.into_iter().map(|test| test.identity).collect::<Vec<_>>(), required);
    }

    #[test]
    fn javascript_deletion_anchor_table_cas_31eb() {
        use crate::factory_worker_check::{javascript, stability, widened};
        let _env = crate::test_support::TestEnvGuard::new();
        let imports = "import { describe, test } from 'node:test';\nimport { appendFileSync } from 'node:fs';\n";
        let callback = "const options = {};\nconst callback = () => {\n  appendFileSync('runs', 'x');\n};\n";
        let registration = "test('kept', options, callback);\n";
        let removed = "test('removed', () => {});\n";
        let inline = "test('kept', () => {\n  appendFileSync('runs', 'x');\n  const removed = 0;\n});\n";
        let kept = format!("{imports}{callback}{registration}");
        let suite = format!("{imports}describe('suite', () => {{\n{callback}{registration}}});\n");
        let cases = [
            ("J1", format!("{removed}{kept}"), kept.clone(), false, false),
            ("J2", format!("{kept}{removed}"), kept.clone(), false, false),
            ("J3", suite.replacen("() => {\n", &format!("() => {{\n{removed}"), 1), suite, false, false),
            ("J4", format!("{imports}{callback}{removed}{registration}"), kept.clone(), false, false),
            ("J5", format!("{imports}{inline}"), format!("{imports}{}", inline.replace("  const removed = 0;\n", "")), true, false),
            ("J6", kept.replace("  appendFileSync", "  const removed = 0;\n  appendFileSync"), kept, false, true),
        ];
        let mut failures = Vec::new();
        for (row, before, after, identified, uncertain) in cases {
            let root = tempfile::tempdir().unwrap();
            let repo = root.path().join("worktrees/worker");
            std::fs::create_dir_all(&repo).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git").current_dir(&repo)
                    .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                    .args(args).output().unwrap();
                assert!(output.status.success(), "{row}: {output:?}");
                String::from_utf8(output.stdout).unwrap().trim().to_string()
            };
            git(&["init", "-q", "-b", "main"]);
            std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
            std::fs::write(repo.join(".gitignore"), "runs\ntarget/\n").unwrap();
            std::fs::write(repo.join("package.json"), "{}").unwrap();
            let path = "a.test.mjs";
            std::fs::write(repo.join(path), before).unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", "base"]);
            let base = git(&["rev-parse", "HEAD"]);
            git(&["checkout", "-qb", "delivery"]);
            std::fs::write(repo.join(path), after).unwrap();
            git(&["commit", "-qam", "delete"]);
            let head = git(&["rev-parse", "HEAD"]);
            let paths = vec![path.to_string()];
            let (tests, scopes) = stability::requirements(&repo, &head, "main", &paths, Some(&[(base, head.clone())])).unwrap();
            let expected_tests = if identified { vec![stability::Identity::exact(path, "kept")] } else { vec![] };
            let expected_scopes = if uncertain { vec![widened::Scope::File(path.into())] } else { vec![] };
            if tests != expected_tests || scopes != expected_scopes {
                failures.push((row, tests, scopes));
                continue;
            }
            let task = crate::types::Task::new("cas-31eb".into(), "fixture".into());
            assert_eq!(park_refusals(root.path(), &repo, &task, &paths, &head, "main", None).is_empty(), !identified && !uncertain, "{row}");
            if identified || uncertain {
                javascript::execute(root.path(), &["node".into(), "--test".into(), path.into()], &repo, &[]).unwrap();
                assert_eq!(std::fs::read(repo.join("runs")).unwrap().len(), if identified { 5 } else { 1 }, "{row}");
            }
            assert!(park_refusals(root.path(), &repo, &task, &paths, &head, "main", None).is_empty(), "{row}");
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// Unknown names widen once through the real runner, then the park consumer
    /// reads that proof. A newer/failing delivery cannot borrow the old pass.
    #[test]
    fn uncertain_js_input_runs_file_or_package_once_before_park_cas_31eb() {
        use crate::factory_worker_check::{javascript, stability, widened, PassingTests};
        let _env = crate::test_support::TestEnvGuard::new();
        for (package, mixed) in [(false, false), (true, false), (false, true), (true, true)] {
            let root = tempfile::tempdir().unwrap();
            let repo = root.path().join("worktrees/worker");
            std::fs::create_dir_all(&repo).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git").current_dir(&repo)
                    .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                    .args(args).output().unwrap();
                assert!(output.status.success(), "{output:?}");
                String::from_utf8(output.stdout).unwrap().trim().to_string()
            };
            git(&["init", "-q", "-b", "main"]);
            std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
            std::fs::write(repo.join(".gitignore"), "runs\ntarget/\n").unwrap();
            std::fs::write(repo.join("package.json"), "{}").unwrap();
            let path = if package { "source.mjs" } else { "a.test.mjs" };
            std::fs::write(repo.join(path), "").unwrap();
            if package { std::fs::write(repo.join("a.test.mjs"), "import './source.mjs';\n").unwrap(); }
            if mixed { std::fs::write(repo.join("known.test.mjs"), "import { test } from 'node:test'; test('known', () => { throw 0; });\n").unwrap(); }
            git(&["add", "."]);
            git(&["commit", "-qm", "base"]);
            let base = git(&["rev-parse", "HEAD"]);
            git(&["checkout", "-qb", "delivery"]);
            let source = "import { test } from 'node:test';\nimport { appendFileSync } from 'node:fs';\nconst name = ['dynamic'].join('');\ntest(name, () => { appendFileSync('runs', 'x'); });\n";
            std::fs::write(repo.join(path), source).unwrap();
            if mixed { std::fs::write(repo.join("known.test.mjs"), "import { test } from 'node:test'; import { appendFileSync } from 'node:fs'; test('known', () => { appendFileSync('runs', 'k'); });\n").unwrap(); }
            git(&["commit", "-qam", "unknown static name"]);
            let head = git(&["rev-parse", "HEAD"]);
            let mut paths = vec![path.to_string()];
            if mixed { paths.push("known.test.mjs".into()); }
            let (tests, scopes) = stability::requirements(&repo, &head, "main", &paths, None).unwrap();
            assert_eq!(tests, if mixed { vec![stability::Identity::exact("known.test.mjs", "known")] } else { vec![] });
            assert_eq!(scopes, [if package { widened::Scope::Package(String::new()) } else { widened::Scope::File(path.into()) }]);
            let task = crate::types::Task::new("cas-31eb".into(), "fixture".into());
            assert!(!park_refusals(root.path(), &repo, &task, &paths, &head, "main", None).is_empty());
            let mut args = vec!["node".into(), "--test".into(), path.into()];
            if mixed { args.push("known.test.mjs".into()); }
            javascript::execute(root.path(), &args, &repo, &[]).unwrap();
            let runs = std::fs::read(repo.join("runs")).unwrap();
            assert_eq!(runs.iter().filter(|byte| **byte == b'x').count(), 1);
            assert_eq!(runs.iter().filter(|byte| **byte == b'k').count(), if mixed { 5 + usize::from(package) } else { 0 });
            assert!(park_refusals(root.path(), &repo, &task, &paths, &head, "main", None).is_empty());
            // The same source-to-runtime uncertainty at a newer failed tip
            // owes a fresh run, and failure records no broadened proof.
            std::fs::write(repo.join(path), source.replace("appendFileSync('runs', 'x');", "throw new Error('seeded');")).unwrap();
            git(&["commit", "-qam", "failing delivery"]);
            let failed = git(&["rev-parse", "HEAD"]);
            assert_eq!(widened::missing(&PassingTests::new(root.path(), &repo, &failed), &scopes, &Default::default()), scopes);
            assert!(javascript::execute(root.path(), &args, &repo, &[]).is_err());
            assert_eq!(widened::missing(&PassingTests::new(root.path(), &repo, &failed), &scopes, &Default::default()), scopes);
            assert!(!park_refusals(root.path(), &repo, &task, &paths, &failed, "main", Some(&[(base, failed.clone())])).is_empty());
        }
        // GH #2175: the widened Playwright producer feeds the same park reader.
        // Only an identical failure at the immutable base may carry a repair link.
        for inherited in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let repo = root.path().join("worktrees/web");
            std::fs::create_dir_all(repo.join("node_modules/.bin")).unwrap();
            let git = |args: &[&str]| {
                let output = std::process::Command::new("git").current_dir(&repo)
                    .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                    .args(args).output().unwrap();
                assert!(output.status.success(), "{output:?}");
                String::from_utf8(output.stdout).unwrap().trim().to_string()
            };
            git(&["init", "-q", "-b", "main"]);
            std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
            std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
            std::fs::write(repo.join("package.json"), "{}").unwrap();
            std::fs::write(repo.join("playwright.config.ts"), "export default {};\n").unwrap();
            let path = "a.spec.ts";
            let source = "import { test } from '@playwright/test';\nconst name = ['existing'].join('');\ntest(name, () => { work(); });\n";
            std::fs::write(repo.join(path), format!("{source}// {}\n", if inherited { "BASE_FAIL" } else { "BASE_PASS" })).unwrap();
            let runner = repo.join("node_modules/.bin/playwright");
            std::fs::write(&runner, r#"#!/usr/bin/env python3
import json, os
from pathlib import Path
failed = 'BASE_FAIL' in Path('a.spec.ts').read_text()
report = {'config': {'rootDir': str(Path.cwd())}, 'errors': [], 'suites': [
    {'title': 'a.spec.ts', 'column': 0, 'specs': [
        {'file': 'a.spec.ts', 'title': 'existing', 'tests': [
            {'projectId': 'journeys', 'expectedStatus': 'passed', 'results': [
                {'status': 'failed' if failed else 'passed', 'retry': 0,
                 'errors': [{'message': 'seeded assertion'}] if failed else []}]}]}], 'suites': []}]}
Path(os.environ['PLAYWRIGHT_JSON_OUTPUT_FILE']).write_text(json.dumps(report))
raise SystemExit(1 if failed else 0)
"#).unwrap();
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            git(&["add", "."]);
            git(&["commit", "-qm", "base"]);
            let base = git(&["rev-parse", "HEAD"]);
            git(&["checkout", "-qb", "delivery"]);
            std::fs::write(repo.join(path), format!("{source}// BASE_FAIL\n// delivery\n")).unwrap();
            git(&["commit", "-qam", "uncertain web delivery"]);
            let head = git(&["rev-parse", "HEAD"]);
            let tasks = crate::store::open_task_store(root.path()).unwrap();
            let mut task = crate::types::Task::new("cas-web".into(), "web fixture".into());
            task.branch = Some("delivery".into());
            tasks.add(&task).unwrap();
            let args = ["playwright".into(), path.into()];
            let result = javascript::execute(root.path(), &args, &repo, &[]);
            assert_eq!(result.is_ok(), inherited, "{result:?}");
            let scopes = [widened::Scope::File(path.into())];
            let missing = widened::missing(&PassingTests::new(root.path(), &repo, &head), &scopes, &Default::default());
            let repairs: Vec<_> = tasks.list(None).unwrap().into_iter()
                .filter(|task| task.labels.iter().any(|label| label == "release-repair")).collect();
            if inherited {
                assert!(missing.is_empty());
                assert_eq!(repairs.len(), 1);
                assert!(repairs[0].description.contains(&base));
                assert!(tasks.get(&task.id).unwrap().notes.contains(&repairs[0].id));
                // A second proof links the same repair instead of filing a duplicate.
                javascript::execute(root.path(), &args, &repo, &[]).unwrap();
                assert_eq!(tasks.list(None).unwrap().iter().filter(|task| task.labels.iter().any(|label| label == "release-repair")).count(), 1);
                assert!(park_refusals(root.path(), &repo, &task, &[path.into()], &head, "main", None).is_empty());
            } else {
                assert!(format!("{:#}", result.unwrap_err()).contains("[G-61]"));
                assert_eq!(missing, scopes);
                assert!(repairs.is_empty());
                let refusals = park_refusals(root.path(), &repo, &task, &[path.into()], &head, "main", None);
                assert!(refusals.iter().any(|refusal| refusal.contains("[G-61]")));
            }
        }

    }

    #[test]
    fn mixed_rust_inputs_keep_identified_stability_beside_widening_cas_31eb() {
        use crate::factory_worker_check::{self, stability, widened, PassingTests};
        let mut failures = Vec::new();
        for (row, known, unknown, selector) in [
            ("R1", Some("src/lib.rs"), None, "fixture"),
            ("R2", Some("tests/native.rs"), None, "fixture"),
            ("R3", None, Some("tests/uncertain.rs"), "fixture"),
            ("R4", Some("src/lib.rs"), Some("src/uncertain.rs"), "fixture"),
            ("R5", Some("src/lib.rs"), Some("tests/uncertain.rs"), "fixture"),
            ("R6", None, Some("src/uncertain.rs"), "fixture"),
            ("R7", None, Some("tests/uncertain.rs"), "fixture@0.1.0"),
        ] {
            let repo = changelog_fixture();
            let root = tempfile::tempdir().unwrap();
            for dir in ["src/fixtures", "tests/fixtures"] { std::fs::create_dir_all(repo.path().join(dir)).unwrap(); }
            let mut manifest = "[package]\nname='fixture'\nversion='0.1.0'\nautotests=false\n".to_string();
            if known == Some("tests/native.rs") { manifest.push_str("[[test]]\nname='native'\npath='tests/native.rs'\n"); }
            if unknown == Some("tests/uncertain.rs") { manifest.push_str("[[test]]\nname='uncertain'\npath='tests/uncertain.rs'\n"); }
            std::fs::write(repo.path().join("Cargo.toml"), manifest).unwrap();
            let prelude = if unknown == Some("src/uncertain.rs") { "mod uncertain;\n" } else { "" };
            std::fs::write(repo.path().join("src/lib.rs"), prelude).unwrap();
            if let Some(path) = known { std::fs::write(repo.path().join(path), format!("{prelude}#[test]\nfn known() {{ let before = 0; }}\n")).unwrap(); }
            if let Some(path) = unknown {
                std::fs::write(repo.path().join(path), "#[path = \"fixtures/a.rs\"] mod nested;\n").unwrap();
                std::fs::write(repo.path().join(path).parent().unwrap().join("fixtures/a.rs"), "#[test]\nfn opaque() {}\n").unwrap();
            }
            let base = commit_changelog_fixture(repo.path(), "base");
            assert!(std::process::Command::new("git").current_dir(repo.path())
                .args(["update-ref", "refs/heads/main", &base]).status().unwrap().success());
            if let Some(path) = known { std::fs::write(repo.path().join(path), format!("{prelude}#[test]\nfn known() {{}}\n")).unwrap(); }
            if let Some(path) = unknown { std::fs::write(repo.path().join(path), "#[path = \"fixtures/\\u{0061}.rs\"] mod nested;\n").unwrap(); }
            let head = commit_changelog_fixture(repo.path(), "delivery");
            let paths = known.into_iter().chain(unknown).map(str::to_string).collect::<Vec<_>>();
            let expected = known.map(|path| stability::Identity::exact(path, "known")).into_iter().collect::<Vec<_>>();
            let expected_scope = widened::Scope::Crate("fixture".into());
            let (tests, scopes) = stability::requirements(repo.path(), &head, "main", &paths, Some(&[(base, head.clone())])).unwrap();
            let expected_scopes = if unknown.is_some() { vec![expected_scope.clone()] } else { vec![] };
            if tests != expected || scopes != expected_scopes {
                failures.push((row, tests, scopes));
                continue;
            }
            factory_worker_check::record_passing_test_for_tests(root.path(), repo.path(), &head, selector, "test(known)");
            let task = crate::types::Task::new("cas-31eb".into(), "fixture".into());
            assert!(!park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None).is_empty(), "{row}");
            if unknown.is_some() {
                // This is the producer's expected crate receipt, independent
                // of the scope the park classifier just returned.
                widened::record(root.path(), repo.path(), &head,
                    widened::Proof { runner: "nextest".into(), scope: expected_scope, reason: "widened:fixture-unclassified-input".into(), configs: Default::default() }, 1).unwrap();
            }
            let passing = PassingTests::new(root.path(), repo.path(), &head);
            assert!(widened::missing(&passing, &scopes, &Default::default()).is_empty(), "{row}");
            assert_eq!(stability::missing(&passing, &tests), tests, "{row}");
            if !tests.is_empty() {
                assert!(!park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None).is_empty(), "{row}");
                stability::record(root.path(), repo.path(), &head, stability::Proof {
                    runner: "nextest".into(), tests, consecutive_passes: 5, elapsed_ms: 1, inactive: Vec::new(),
                }).unwrap();
            }
            assert!(park_refusals(root.path(), repo.path(), &task, &paths, &head, "main", None).is_empty(), "{row}");
        }
        assert!(failures.is_empty(), "{failures:?}");
    }

    /// GH #2266, #2224: a migration rename, a catalogue edit and a fixture
    /// edit widen the package; park names the specs that read those inputs
    /// in one runnable command and accepts their whole-file passes for it.
    #[test]
    fn package_proof_resolves_and_accepts_covering_specs_gh2266() {
        use crate::factory_worker_check::{PassingTests, stability, widened};
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("worktrees/worker");
        let write = |path: &str, body: &str| {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        };
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(&repo)
                .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let specs = ["apps/backend/src/__scenarios__/acceptance.spec.ts", "apps/backend/test/migration-timestamps.spec.ts",
            "apps/backend/test/patterns.spec.ts"];
        write("apps/backend/package.json", "{}");
        write("apps/backend/prisma/migrations/1_init/migration.sql", "CREATE TABLE a (id int);\n");
        write("apps/backend/src/__scenarios__/scenarios.json", "[[\"sms\", false]]");
        write("apps/backend/test/fixtures/patterns.ts", "export const pattern = /^a$/;\n");
        write(specs[0], "import cases from './scenarios.json';\nfor (const [name] of cases) it(name, () => {});\n");
        write(specs[1], "import { readdirSync } from 'fs';\nimport { join } from 'path';\n\
            it('orders migrations', () => { readdirSync(join(__dirname, '../prisma/migrations')); });\n");
        write(specs[2], "import { pattern } from './fixtures/patterns';\nit('matches', () => { pattern.test('a'); });\n");
        write("apps/backend/src/unrelated.spec.ts", "import { x } from '../lib/x';\nit('unrelated', () => { void x; });\n");
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]); git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "delivery"]);
        git(&["mv", "apps/backend/prisma/migrations/1_init", "apps/backend/prisma/migrations/2_init"]);
        write("apps/backend/src/__scenarios__/scenarios.json", "[[\"sms\", false], [\"email\", true]]");
        write("apps/backend/test/fixtures/patterns.ts", "export const pattern = /^a+$/;\n");
        git(&["commit", "-qam", "rename migration, extend catalogue, widen fixture"]);
        std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
        let head = git(&["rev-parse", "HEAD"]);
        let paths = ["apps/backend/prisma/migrations/2_init/migration.sql", "apps/backend/src/__scenarios__/scenarios.json",
            "apps/backend/test/fixtures/patterns.ts"].map(String::from).to_vec();
        let task = Task::new("cas-5afc".into(), "covering spec proof".into());
        let (_, scopes) = stability::requirements(&repo, &head, "main", &paths, None).unwrap();
        let package = widened::Scope::Package("apps/backend".into());
        // GH #2221: the changed catalogue also owes its importing spec a whole-file run.
        assert_eq!(scopes, [widened::Scope::File(specs[0].into()), package.clone()]);
        let covering = widened::covering(&repo, &head, &paths, &scopes);
        let found = covering.get(&package).unwrap();
        assert_eq!(found.specs, specs.map(String::from));
        assert!(found.unresolved.is_empty(), "every changed input is read by a covering spec");
        // Every prescribed package command targets exactly the covering specs.
        let commands = widened::commands(&PassingTests::new(root.path(), &repo, &head), &paths, &[package.clone()], &covering);
        assert!(!commands.is_empty());
        for command in &commands {
            let (_, args) = command.split_once(" -- ").unwrap();
            assert_eq!(args.split_whitespace().filter(|arg| arg.ends_with(".ts")).collect::<Vec<_>>(), specs, "{command}");
        }
        let g61 = |head: &str| park_refusals(root.path(), &repo, &task, &paths, head, "main", None).iter()
            .any(|reply| crate::gates::codes_in(reply).contains(&"G-61"));
        let pass = |at: &str, spec: &str| widened::record(root.path(), &repo, at, widened::Proof {
            runner: "jest".into(), scope: widened::Scope::File(spec.into()), reason: "widened:no static changed-test selection".into(),
            configs: Default::default(),
        }, 1).unwrap();
        assert!(g61(&head));
        pass(&head, specs[0]);
        pass(&head, specs[1]);
        assert!(g61(&head), "two of three covering specs must not prove the package");
        pass(&head, specs[2]);
        assert!(!g61(&head));

        // A mixed package: the same inputs plus seed data that no spec reads.
        // The covering specs say nothing about it, so their passes do not
        // prove the package; the refusal names the input and keeps the
        // package's own proof as the way out.
        let unread = "apps/backend/prisma/seeds/seed.sql";
        write(unread, "INSERT INTO a VALUES (1);\n");
        git(&["add", "."]); git(&["commit", "-qm", "seed data no spec reads"]);
        let mixed = git(&["rev-parse", "HEAD"]);
        let paths: Vec<String> = paths.into_iter().chain([unread.to_string()]).collect();
        let (_, scopes) = stability::requirements(&repo, &mixed, "main", &paths, None).unwrap();
        let covering = widened::covering(&repo, &mixed, &paths, &scopes);
        let found = covering.get(&package).unwrap();
        assert_eq!((found.specs.as_slice(), found.unresolved.as_slice()), (&specs.map(String::from)[..], &[unread.to_string()][..]));
        for spec in specs { pass(&mixed, spec); }
        let refusals = park_refusals(root.path(), &repo, &task, &paths, &mixed, "main", None);
        assert!(refusals.iter().any(|reply| crate::gates::codes_in(reply).contains(&"G-61") && reply.contains(unread)), "{refusals:?}");
        assert_eq!(widened::missing(&crate::factory_worker_check::PassingTests::new(root.path(), &repo, &mixed), &scopes, &covering), [package.clone()]);
        let commands = widened::commands(&PassingTests::new(root.path(), &repo, &mixed), &paths, &[package.clone()], &covering);
        assert!(!commands.is_empty());
        for command in &commands {
            let args = shell_words::split(command.split_once(" -- ").unwrap().1).unwrap();
            assert!(specs.iter().all(|spec| !args.iter().any(|arg| arg == spec)), "{command}: a covering spec is not the package's proof");
        }
    }

    /// Framework boundary: implement the import-graph selection predicate,
    /// report unrelated failures and report an empty selection as empty.
    #[cfg(unix)]
    const FIXTURE_RUNNER: &str = r#"#!/usr/bin/env python3
import json, pathlib, re, sys
a = sys.argv[1:]
test_root = pathlib.Path.cwd()
if '--config' in a:
    config = pathlib.Path(a[a.index('--config') + 1])
    configured_root = re.search(r"\broot:\s*['\"]([^'\"]+)", config.read_text())
    if configured_root: test_root = config.parent / configured_root[1]
specs = sorted(test_root.rglob('*.spec.ts'))
if '--listTests' in a:
    print(json.dumps([str(p) for p in specs]))
    sys.exit(0)
def depends(spec, target, seen=None):
    seen = set() if seen is None else seen
    if spec in seen: return False
    seen.add(spec)
    if spec == target: return True
    for name in re.findall(r"from\s+['\"]([^'\"]+)", spec.read_text()):
        dependency = (spec.parent / name).resolve()
        if not dependency.suffix: dependency = dependency.with_suffix('.ts')
        if dependency.is_file() and depends(dependency, target, seen): return True
    return False
if '--findRelatedTests' in a:
    target = pathlib.Path(a[a.index('--findRelatedTests') + 1]).resolve()
    specs = [p for p in specs if depends(p, target)]
elif 'related' in a:
    target = (test_root / a[-1]).resolve()
    specs = [p for p in specs if depends(p, target)]
elif '--runTestsByPath' in a:
    target = pathlib.Path(a[a.index('--runTestsByPath') + 1]).resolve()
    specs = [p for p in specs if p == target]
def assertions(p):
    # Real Jest registers one test per expanded it.each row, titled by its interpolation.
    status = 'failed' if 'throw 0' in p.read_text() else 'passed'
    names = []
    for table, title in re.findall(r"it\.each\(\[(.*?)\]\)\('([^']*)'", p.read_text()):
        for row in re.findall(r"\{([^}]*)\}", table):
            name = title
            for key, value in re.findall(r"(\w+):\s*'([^']*)'", row):
                name = name.replace('$' + key, value)
            names.append(name)
    return [{'fullName': name, 'status': status} for name in names or ['claims']]
results = [{'name': str(p), 'status': 'failed' if 'throw 0' in p.read_text() else 'passed',
    'assertionResults': assertions(p)} for p in specs]
ok = bool(results) and all(r['status'] == 'passed' for r in results)
pathlib.Path(a[a.index('--outputFile') + 1]).write_text(json.dumps({'success': ok, 'testResults': results}))
pathlib.Path('target').mkdir(exist_ok=True)
with pathlib.Path('target/executed.jsonl').open('a') as out: out.write(json.dumps([str(p) for p in specs]) + '\n')
sys.exit(0 if ok else 1)
"#;

    /// Source advice, native related selection and the park reader share one
    /// fixture; each runner must exclude an unrelated failure in its package.
    #[cfg(unix)]
    fn source_related_tests_fixture(runner: &str, source: &str, changed: &str) {
        use crate::factory_worker_check::{javascript, stability, widened, PassingTests};
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.remove("CAS_SESSION_ID");
        let root = tempfile::tempdir().unwrap();
        // cas-253d: the fixture runner reports resolved paths; macOS temp
        // roots (/tmp, /var) are symlinks, so keep the checkout physical.
        let repo = root.path().canonicalize().unwrap().join("worktrees/worker");
        let write = |path: &str, body: &str| {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        };
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(&repo)
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let package = source.split('/').take(2).collect::<Vec<_>>().join("/");
        let (test_dir, parent) = if runner == "vitest" { ("app/test", "../../") } else { ("test", "../") };
        let consumer = format!("{package}/{test_dir}/consumer.spec.ts");
        let consumer_source = format!("import {{ value }} from '{parent}{}';\nit('claims', () => {{ void value; }});\n", source.strip_prefix(&format!("{package}/")).unwrap());
        write(".gitignore", "node_modules/\ntarget/\n");
        write(&format!("{package}/package.json"), &format!(r#"{{"devDependencies":{{"{runner}":"3"}}}}"#));
        write(&format!("{package}/{runner}.config.js"), if runner == "vitest" {
            "module.exports = { root: 'app' };\n"
        } else { "module.exports = {};\n" });
        write(source, "export const value = 1;\n");
        // The consuming spec is elsewhere; a sibling-name heuristic cannot find it.
        write(&consumer, &consumer_source);
        write(&format!("{package}/{test_dir}/unrelated.spec.ts"), "it('unrelated', () => { throw 0; });\n");
        write(&format!("{package}/node_modules/.bin/{runner}"), FIXTURE_RUNNER);
        std::fs::set_permissions(repo.join(format!("{package}/node_modules/.bin/{runner}")), std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]); git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "delivery"]);
        write(source, changed);
        git(&["commit", "-qam", "change source"]);
        std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
        let head = git(&["rev-parse", "HEAD"]);
        let paths = vec![source.to_owned()];
        let task = Task::new("cas-1587".into(), "related source proof".into());
        let scopes = vec![widened::scope(&repo, source)];
        let commands = widened::commands(&PassingTests::new(root.path(), &repo, &head), &paths, &scopes, &Default::default());
        assert_eq!(commands.len(), 1);
        // These bytes are the runnable command consumed by worker-check.
        let (_, command) = commands[0].split_once(" -- ").unwrap();
        let args = shell_words::split(command).unwrap();
        if runner == "jest" {
            assert!(args.iter().any(|arg| arg == "--findRelatedTests"), "{commands:?}");
        } else {
            assert!(args.starts_with(&["vitest".into(), "related".into(), "--run".into()]), "{commands:?}");
        }
        let (tests, required) = stability::requirements(&repo, &head, "main", &paths, None).unwrap();
        assert!(tests.is_empty());
        assert_eq!(required, scopes);
        let held = park_refusals(root.path(), &repo, &task, &paths, &head, "main", None);
        assert!(held.iter().any(|reply| crate::gates::codes_in(reply).contains(&"G-61")));
        javascript::execute(root.path(), &args, &repo, &[]).unwrap();
        let runs = std::fs::read_to_string(repo.join(format!("{package}/target/executed.jsonl"))).unwrap();
        let executed = runs.lines().map(|line| serde_json::from_str::<Vec<String>>(line).unwrap()).collect::<Vec<_>>();
        assert_eq!(executed, [vec![repo.join(&consumer).to_str().unwrap().to_owned()]]);
        assert!(widened::missing(&PassingTests::new(root.path(), &repo, &head), &scopes, &Default::default()).is_empty());
        assert!(park_refusals(root.path(), &repo, &task, &paths, &head, "main", None).is_empty());
        // A receipt for this source cannot cover a second source in the package.
        let other_source = format!("{package}/src/other.ts");
        write(&other_source, "export const value = 3;\n");
        let other = widened::scope(&repo, &other_source);
        assert_eq!(widened::missing(&PassingTests::new(root.path(), &repo, &head), &[other.clone()], &Default::default()), [other]);
        std::fs::remove_file(repo.join(&other_source)).unwrap();
        // A failing related spec must withdraw the old proof at the new tip.
        write(&consumer, &consumer_source.replace("void value;", "throw 0;"));
        git(&["commit", "-qam", "related failure"]);
        let failed = git(&["rev-parse", "HEAD"]);
        assert!(javascript::execute(root.path(), &args, &repo, &[]).is_err());
        assert_eq!(widened::missing(&PassingTests::new(root.path(), &repo, &failed), &scopes, &Default::default()), scopes);
        assert!(!park_refusals(root.path(), &repo, &task, &paths, &failed, "main", None).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn jest_source_related_tests_command_and_park_receipt_gh2257() {
        source_related_tests_fixture("jest", "apps/backend/src/conversations/v2/make-it-mine-request.ts",
            "export function value(it: { claim: () => number }) { return it.claim(); }\n");
    }

    /// GH #2252: an unclassifiable test helper widens to its package, beside a
    /// changed Prisma schema that sorts first. Every focused command the
    /// refusal prints must be one worker-check accepts; running it satisfies park.
    #[cfg(unix)]
    #[test]
    fn jest_package_scope_command_runs_and_proves_gh2252() {
        use crate::factory_worker_check::{javascript, stability, widened, PassingTests};
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_BUILD_GUARD", "off");
        env.remove("CAS_SESSION_ID");
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().canonicalize().unwrap().join("worktrees/worker");
        let write = |path: &str, body: &str| {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        };
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").current_dir(&repo)
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let schema = "apps/backend/prisma/schema.prisma";
        write(".gitignore", "node_modules/\ntarget/\n");
        let helper = "apps/backend/test/caption-cases.ts";
        // Both runners declared and no config: no single runner owns the helper.
        write("apps/backend/package.json", r#"{"devDependencies":{"jest":"29","vitest":"3"}}"#);
        write(schema, "model Post { id Int @id }\n");
        write(helper, "export const check = (name: string) => it(name, () => {});\n");
        write("apps/backend/src/comparison.spec.ts", "it.each([{ delivery: 'spoken' }, { delivery: 'text_on_screen' }])('ignores caption-only edits for $delivery', () => {});\n");
        write("apps/backend/node_modules/.bin/jest", FIXTURE_RUNNER);
        std::fs::set_permissions(repo.join("apps/backend/node_modules/.bin/jest"), std::fs::Permissions::from_mode(0o755)).unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]); git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "delivery"]);
        write(schema, "model Post { id Int @id\n  caption String }\n");
        write(helper, "export const check = (name: string) => it(name, async () => {});\n");
        git(&["commit", "-qam", "change schema and helper"]);
        std::fs::write(root.path().join("config.toml"), "[factory]\nepic_base_branch='main'\n").unwrap();
        let head = git(&["rev-parse", "HEAD"]);
        let paths = vec![schema.to_owned(), helper.to_owned()];
        let (tests, scopes) = stability::requirements(&repo, &head, "main", &paths, None).unwrap();
        assert!(tests.is_empty());
        assert_eq!(scopes, [widened::Scope::Package("apps/backend".into())]);
        let commands = widened::commands(&PassingTests::new(root.path(), &repo, &head), &paths, &scopes, &Default::default());
        assert!(!commands.is_empty());
        for command in &commands {
            let (_, command) = command.split_once(" -- ").unwrap();
            let args = shell_words::split(command).unwrap();
            assert!(crate::factory_worker_check::allowed_args(&args), "{command}");
            if args[0] == "jest" { javascript::execute(root.path(), &args, &repo, &[]).unwrap(); }
        }
        assert!(widened::missing(&PassingTests::new(root.path(), &repo, &head), &scopes, &Default::default()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn vitest_helper_related_tests_command_and_park_receipt_gh2265() {
        for source in ["apps/frontend/test/edit-part.ts", "apps/frontend/__tests__/make-it-mine.ts"] {
            source_related_tests_fixture("vitest", source, "export const value = 2;\n");
        }
    }

    fn changelog_fixture() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::create_dir_all(repo.path().join("hub-web/src")).unwrap();
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 1;\n").unwrap();
        std::fs::write(repo.path().join("CHANGELOG.md"), "# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-01-01\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "delivery"]);
        repo
    }

    fn commit_changelog_fixture(repo: &Path, subject: &str) -> String {
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["add", "."]);
        git(&["commit", "--allow-empty", "-qm", subject]);
        git(&["rev-parse", "HEAD"])
    }



    #[test]
    fn product_source_delivery_requires_a_titled_unreleased_change() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        for path in [
            "cas-cli/src/main.rs",
            "cas-cli/src/builtins/skills/cas-worker/SKILL.md",
            "crates/cas-store/src/lib.rs",
            "hub-web/src/app.ts",
            "scripts/release.sh",
        ] {
            assert!(product_source_path(path), "{path} should require release notes");
        }
        for path in ["docs/guide.md", "cas-cli/tests/cli.rs", "scripts/test-checks.py"] {
            assert!(!product_source_path(path), "{path} is not shipped product source");
        }

        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        let source_only = commit_changelog_fixture(repo.path(), "product change");
        let changed = vec!["hub-web/src/app.ts".to_string()];
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &source_only, "main", None)
            .expect("product source without a changelog entry must be refused");
        assert!(refusal.contains("CHANGELOG [Unreleased] REQUIRED"), "{refusal}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- Generic status entry.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let untitled = commit_changelog_fixture(repo.path(), "add an untitled note");
        let changed = vec!["CHANGELOG.md".to_string(), "hub-web/src/app.ts".to_string()];
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &untitled, "main", None)
            .expect("an untitled Unreleased bullet is not release evidence");
        assert!(refusal.contains("CHANGELOG [Unreleased] REQUIRED"), "{refusal}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- **Status updates stay current** — Status now reflects the latest response.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let titled = commit_changelog_fixture(repo.path(), "add release note");
        let changelog_path = repo.path().join("CHANGELOG.md");
        let text = std::fs::read_to_string(changelog_path).unwrap();
        assert_eq!(titled_unreleased_entries(&text).unwrap().len(), 1);
        assert!(has_titled_unreleased_delta(repo.path(), &titled, "main", None, &task.id));
        let accepted = park_refusal(cas_root.path(), repo.path(), &task, &changed, &titled, "main", None);
        assert!(accepted.is_none(), "{accepted:?}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- **Status updates stay current** — Status now follows the current response.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let extended = commit_changelog_fixture(repo.path(), "extend release note");
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &extended, "main", None).is_none());
    }

    /// cas-f2be: with `changelog.d/` tracked, a delivery's entry must be its
    /// own fragment file. A direct `CHANGELOG.md` entry is refused because
    /// GitHub's merge queue conflicts two of them at the top of the section.
    #[test]
    fn fragment_project_credits_only_fragment_entries() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        std::fs::create_dir_all(repo.path().join("changelog.d")).unwrap();
        std::fs::write(repo.path().join("changelog.d/README.md"), "### Fixed — <area>\n\n- **Title** — What changed.\n").unwrap();
        commit_changelog_fixture(repo.path(), "keep fragments");
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- **Status updates stay current** — Status follows the latest response.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let direct = commit_changelog_fixture(repo.path(), "direct entry");
        let changed = vec!["CHANGELOG.md".to_string(), "hub-web/src/app.ts".to_string()];
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &direct, "main", None)
            .expect("a direct CHANGELOG entry is refused once fragments exist");
        assert!(refusal.contains("`changelog.d/cas-changelog-fixture.md`"), "{refusal}");

        std::fs::write(
            repo.path().join("changelog.d/cas-changelog-fixture.md"),
            "### Fixed — Commander\n\n- **Badges show versions** — Each badge names its version.\n",
        )
        .unwrap();
        let fragment = commit_changelog_fixture(repo.path(), "fragment entry");
        let changed = vec!["changelog.d/cas-changelog-fixture.md".to_string(), "hub-web/src/app.ts".to_string()];
        let accepted = park_refusal(cas_root.path(), repo.path(), &task, &changed, &fragment, "main", None);
        assert!(accepted.is_none(), "{accepted:?}");
    }

    #[test]
    fn corrected_fragment_counts_at_final_delivery_tip_gh1690() {
        use super::super::{TaskCommitIdentity, TaskCommitReceiptWindow, task_attribution};
        use cas_store::TaskStore;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let repo = changelog_fixture();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["branch", "-m", "factory/worker"]);
        let mut task = Task::new("cas-8c58".into(), "worker account preference".into());
        std::fs::create_dir_all(repo.path().join("changelog.d")).unwrap();
        std::fs::write(repo.path().join("changelog.d/README.md"), "Per-task release notes.\n").unwrap();
        let base = commit_changelog_fixture(repo.path(), "enable fragments");
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        let fragment = "changelog.d/cas-8c58.md";
        std::fs::write(repo.path().join(fragment),
            "### Honor the configured Codex worker account\n\nUse the configured account before recommendations.\n").unwrap();
        let original = commit_changelog_fixture(repo.path(),
            "fix: honor configured Codex worker account before recommendations");
        task.status = cas_types::TaskStatus::InProgress;
        task.assignee = Some("worker".into());
        let target = cas_types::WorkTarget {
            repo_selector: "project:fixture".into(), target_branch: "main".into(),
        };
        task.deliverables.work_target = Some(target.clone());
        task.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: target, assignee: "worker".into(), branch: "factory/worker".into(),
            base_sha: base.clone(),
        });
        task.deliverables.factory_branch_anchor = Some(original.clone());
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::DateTime::UNIX_EPOCH,
            task_floor: chrono::DateTime::UNIX_EPOCH,
            basis: "fixture",
            identity: TaskCommitIdentity {
                superseded_commits: Vec::new(),
                approved_merge_parents: Vec::new(),
                start_base: Some(base.clone()), task_id: Some(task.id.clone()),
                known_commits: vec![original.clone()],
            },
        };
        let refusal = |head: &str| {
            let ranges = task_attribution::range_bounds(repo.path(), "main", &window, Some(head)).unwrap();
            let paths = task_attribution::paths(repo.path(), "main", &window, Some(head)).unwrap();
            assert!(paths.iter().any(|path| path == fragment));
            changelog_refusal(repo.path(), &task, &paths, head, "main", Some(&ranges))
        };
        assert!(refusal(&original).is_some());
        std::fs::write(repo.path().join(fragment),
            "### Fixed — Worker accounts\n\n- **Honor the configured Codex worker account** — Use the configured account before recommendations.\n").unwrap();
        let corrected = commit_changelog_fixture(repo.path(),
            "docs: use the required worker account changelog fragment format");
        let origin = tempfile::tempdir().unwrap();
        let output = std::process::Command::new("git")
            .args(["init", "-q", "--bare"]).current_dir(origin.path()).output().unwrap();
        assert!(output.status.success());
        git(&["remote", "add", "origin", origin.path().to_str().unwrap()]);
        git(&["push", "-q", "origin", "main", "factory/worker"]);
        let root = repo.path().join(".cas");
        std::fs::create_dir_all(&root).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        let store = cas_store::SqliteTaskStore::open(&root).unwrap();
        store.init().unwrap();
        store.add(&task).unwrap();
        let parking = core.advance_commit_time_anchor_before_park(
            &store, &task, repo.path(), "main", Some("factory/worker"), Some(&corrected),
        );
        let inspected = parking.deliverables.factory_branch_anchor.unwrap();
        assert_eq!(inspected, corrected);
        assert!(refusal(&inspected).is_none());
        assert_eq!(store.get(&task.id).unwrap().deliverables.factory_branch_anchor.as_deref(), Some(original.as_str()));

        // A stale caller can still inspect the old malformed snapshot. Its
        // diagnostic must identify the inspected commit and attributed bounds.
        let stale = refusal(&original).unwrap();
        assert!(stale.contains(&original));
        assert!(stale.contains(&base));
    }

    #[test]
    fn returned_delivery_merge_resolution_fragment_counts_cas_3623() {
        use super::super::{TaskCommitIdentity, TaskCommitReceiptWindow, task_attribution};
        let repo = changelog_fixture();
        let task = Task::new("cas-c568".into(), "returned delivery".into());
        let fragment = "changelog.d/cas-c568.md";
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        std::fs::create_dir_all(repo.path().join("changelog.d")).unwrap();
        std::fs::write(repo.path().join("changelog.d/README.md"), "Per-task release notes.\n").unwrap();
        let base = commit_changelog_fixture(repo.path(), "enable fragments");
        git(&["branch", "-f", "main", &base]);
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        std::fs::write(repo.path().join(fragment),
            "### Fixed — Factory\n\n- **Quiet expiry of old transcript mirrors** — Previous sessions expire quietly.\n").unwrap();
        let parked = commit_changelog_fixture(repo.path(), "fix: quiet old mirrors (cas-c568)");
        git(&["checkout", "-q", "main"]);
        std::fs::write(repo.path().join("changelog.d/cas-beef.md"),
            "### Fixed — Factory\n\n- **Foreign release note** — Another task's delivery.\n").unwrap();
        commit_changelog_fixture(repo.path(), "fix: target work (cas-beef)");
        git(&["checkout", "-q", "delivery"]);
        git(&["merge", "-q", "--no-ff", "--no-commit", "main"]);
        std::fs::write(repo.path().join(fragment),
            "### Fixed — Factory\n\n- **Preserve current-session delivery warnings** — Current mirrors still warn.\n").unwrap();
        let merge = commit_changelog_fixture(repo.path(), "merge: preserve current-session warnings");
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 3;\n").unwrap();
        let head = commit_changelog_fixture(repo.path(), "fix: retain decision warnings (cas-c568)");
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::DateTime::UNIX_EPOCH,
            task_floor: chrono::DateTime::UNIX_EPOCH,
            basis: "fixture",
            identity: TaskCommitIdentity {
                superseded_commits: Vec::new(),
                approved_merge_parents: Vec::new(),
                start_base: Some(base.clone()), task_id: Some(task.id.clone()),
                known_commits: vec![parked.clone()],
            },
        };
        let ranges = task_attribution::range_bounds(repo.path(), "main", &window, Some(&head)).unwrap();
        assert_eq!(ranges, vec![(base, parked), (merge, head.clone())]);
        let paths = task_attribution::paths(repo.path(), "main", &window, Some(&head)).unwrap();
        assert!(!paths.iter().any(|path| path == "changelog.d/cas-beef.md"));
        let imported = Task::new("cas-beef".into(), "imported fragment without delivery provenance".into());
        assert!(changelog_refusal(repo.path(), &imported, &paths, &head, "main", Some(&ranges)).is_some());
        assert!(changelog_refusal(repo.path(), &task, &paths, &head, "main", Some(&ranges)).is_none());

        // The imported note cannot replace missing or malformed task evidence.
        for contents in [None, Some("### Fixed — Factory\n\n- Untitled change.\n")] {
            if let Some(contents) = contents {
                std::fs::write(repo.path().join(fragment), contents).unwrap();
            } else {
                std::fs::remove_file(repo.path().join(fragment)).unwrap();
            }
            let invalid = commit_changelog_fixture(repo.path(), "invalid fragment control (cas-c568)");
            let ranges = task_attribution::range_bounds(repo.path(), "main", &window, Some(&invalid)).unwrap();
            let paths = task_attribution::paths(repo.path(), "main", &window, Some(&invalid)).unwrap();
            assert!(changelog_refusal(repo.path(), &task, &paths, &invalid, "main", Some(&ranges)).is_some());
        }
    }

    #[test]
    fn merge_authored_task_fragment_counts_without_crediting_imports_cas_28f8() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let task = Task::new("cas-28f8".into(), "integration delivery".into());
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        std::fs::create_dir_all(repo.path().join("changelog.d")).unwrap();
        std::fs::write(repo.path().join("changelog.d/README.md"), "Per-task release notes.\n").unwrap();
        commit_changelog_fixture(repo.path(), "enable fragments");
        git(&["branch", "foreign"]);
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        commit_changelog_fixture(repo.path(), "integration source change");
        git(&["checkout", "-q", "foreign"]);
        std::fs::write(
            repo.path().join("changelog.d/cas-beef.md"),
            "### Fixed — Factory\n\n- **Foreign delivery** — Another task's entry.\n",
        ).unwrap();
        commit_changelog_fixture(repo.path(), "foreign release note");
        git(&["checkout", "-q", "delivery"]);
        git(&["merge", "-q", "--no-ff", "--no-edit", "foreign"]);
        let imported = commit_changelog_fixture(repo.path(), "generated continuation");
        let mut changed = vec!["hub-web/src/app.ts".into(), "changelog.d/cas-beef.md".into()];
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &imported, "main", None).is_some(),
            "an imported foreign fragment cannot satisfy the integration task's changelog");

        // Return to the merge and author the task's fragment in its result,
        // preserving both parents; neither parent contains this fragment.
        git(&["checkout", "-q", "-b", "authored", "HEAD~1"]);
        std::fs::write(
            repo.path().join("changelog.d/cas-28f8.md"),
            "### Fixed — Factory\n\n- **Integration release note** — The integration task records its own change.\n",
        ).unwrap();
        git(&["add", "changelog.d/cas-28f8.md"]);
        git(&["commit", "-q", "--amend", "--no-edit"]);
        std::fs::write(repo.path().join("generated.txt"), "refreshed history\n").unwrap();
        let head = commit_changelog_fixture(repo.path(), "generated continuation");
        changed.push("changelog.d/cas-28f8.md".into());
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &head, "main", None);
        assert!(refusal.is_none(), "a fragment authored by the merge survives continuation: {refusal:?}");
    }

    /// cas-2a65: through the close's task-attributed ranges, an entry a lane
    /// receives by merging its predecessor's same-task branch counts. Another
    /// task's merged entry beside the lane's own untitled bullet, and the
    /// target's entry from a refresh, do not.
    #[test]
    fn changelog_entry_counts_only_from_task_attributed_ranges() {
        use super::super::{TaskCommitIdentity, TaskCommitReceiptWindow, task_attribution};
        let repo = changelog_fixture();
        let task = Task::new("cas-a1b2".into(), "delivery".into());
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::DateTime::UNIX_EPOCH,
            task_floor: chrono::DateTime::UNIX_EPOCH,
            basis: "fixture",
            identity: TaskCommitIdentity { superseded_commits: Vec::new(), approved_merge_parents: Vec::new(), start_base: None, task_id: Some(task.id.clone()), known_commits: vec![] },
        };
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        let changelog = |bullets: &str| std::fs::write(
            repo.path().join("CHANGELOG.md"),
            format!("# Changelog\n\n## [Unreleased]\n\n### Added — Commander\n\n{bullets}\n## [1.0.0] - 2026-01-01\n"),
        ).unwrap();
        let refusal = |branch: &str| {
            git(&["checkout", "-q", branch]);
            let head = commit_changelog_fixture(repo.path(), "tip");
            let ranges = task_attribution::range_bounds(repo.path(), "main", &window, None).unwrap();
            let changed = vec!["CHANGELOG.md".to_string(), "hub-web/src/app.ts".to_string()];
            changelog_refusal(repo.path(), &task, &changed, &head, "main", Some(&ranges))
        };

        git(&["checkout", "-qb", "predecessor", "main"]);
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        changelog("- **Badges show versions** — It is visible.\n");
        commit_changelog_fixture(repo.path(), "feat: badges (cas-a1b2)");
        git(&["checkout", "-q", "delivery"]);
        git(&["merge", "-q", "--no-ff", "--no-edit", "predecessor"]);
        assert!(refusal("delivery").is_none(), "the predecessor's same-task entry counts");

        git(&["checkout", "-qb", "foreign", "main"]);
        changelog("- **Another task's change** — It is visible.\n");
        commit_changelog_fixture(repo.path(), "feat: other lane (cas-beef)");
        git(&["checkout", "-qb", "untitled", "main"]);
        git(&["merge", "-q", "--no-ff", "--no-edit", "foreign"]);
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 3;\n").unwrap();
        changelog("- **Another task's change** — It is visible.\n- Lane note without a title.\n");
        commit_changelog_fixture(repo.path(), "fix: lane work (cas-a1b2)");
        assert!(refusal("untitled").is_some(), "another task's merged entry is not this delivery's");

        git(&["checkout", "-qb", "refresh", "main"]);
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 4;\n").unwrap();
        commit_changelog_fixture(repo.path(), "fix: source only (cas-a1b2)");
        git(&["checkout", "-q", "main"]);
        changelog("- **Target change** — It is visible.\n");
        commit_changelog_fixture(repo.path(), "target entry");
        git(&["checkout", "-q", "refresh"]);
        git(&["merge", "-q", "--no-edit", "main"]);
        assert!(refusal("refresh").is_some(), "a target refresh cannot claim the target's entry");
    }

    #[test]
    fn adopted_delivery_retains_original_fragment_after_restart_cas_2a80() {
        use super::super::{TaskCommitIdentity, TaskCommitReceiptWindow, task_attribution};
        let repo = changelog_fixture();
        let task = Task::new("cas-a1b2".into(), "adopted delivery".into());
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(repo.path()).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        // The original worker parked its source and fragment on a side branch.
        git(&["checkout", "-qb", "factory/old-cas-a1b2", "main"]);
        std::fs::create_dir(repo.path().join("changelog.d")).unwrap();
        let fragment = "changelog.d/cas-a1b2.md";
        std::fs::write(repo.path().join(fragment), "### Fixed — Factory\n\n- **Replacement assignment** — Retire the previous assignment.\n").unwrap();
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        let original = commit_changelog_fixture(repo.path(), "fix(cas-a1b2): release old assignment");
        git(&["checkout", "main"]);
        std::fs::write(repo.path().join("upstream.txt"), "new train\n").unwrap();
        let start = commit_changelog_fixture(repo.path(), "train advance");
        git(&["checkout", "-qb", "factory/new-cas-a1b2"]);
        // Branch names embed the task id but are not whole-token task claims.
        git(&["merge", "-q", "--no-ff", "--no-edit", "factory/old-cas-a1b2"]);
        std::fs::write(repo.path().join("README.md"), "assignment ownership\n").unwrap();
        commit_changelog_fixture(repo.path(), "document ownership");
        std::fs::write(repo.path().join("diagnostic.txt"), "temporary probe\n").unwrap();
        let diagnostic = commit_changelog_fixture(repo.path(), "diagnose(a1b2): inspect assignment");
        git(&["revert", "--no-edit", &diagnostic]);
        git(&["checkout", "main"]);
        std::fs::write(repo.path().join("target-only.txt"), "unrelated target change\n").unwrap();
        commit_changelog_fixture(repo.path(), "target refresh");
        git(&["checkout", "factory/new-cas-a1b2"]);
        git(&["merge", "-q", "--no-ff", "--no-edit", "main"]);
        let head = git(&["rev-parse", "HEAD"]);
        let window = TaskCommitReceiptWindow {
            supervisor_override_reason: None,
            not_before: chrono::Utc::now() + chrono::Duration::hours(1),
            task_floor: chrono::DateTime::UNIX_EPOCH,
            basis: "replacement worker restarted after adoption",
            identity: TaskCommitIdentity {
                task_id: Some(task.id.clone()),
                start_base: Some(start),
                known_commits: vec![original, diagnostic, head.clone()],
                ..Default::default()
            },
        };
        let changed = task_attribution::paths(repo.path(), "main", &window, Some(&head)).unwrap();
        assert!(changed.iter().any(|path| path == fragment), "{changed:?}");
        assert!(changed.iter().any(|path| path == "hub-web/src/app.ts"), "{changed:?}");
        assert!(!changed.iter().any(|path| path == "target-only.txt"), "{changed:?}");
        let ranges = task_attribution::range_bounds(repo.path(), "main", &window, Some(&head)).unwrap();
        assert!(changelog_refusal(repo.path(), &task, &changed, &head, "main", Some(&ranges)).is_none());
    }

    #[test]
    fn reasoned_no_changelog_decision_exempts_source_delivery() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        let head = commit_changelog_fixture(repo.path(), "internal test-only adjustment");
        let mut task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog:".into();
        assert!(changelog_refusal(
            repo.path(),
            &task,
            &["hub-web/src/app.ts".into()],
            &head,
            "main",
            None,
        )
        .is_some(), "an empty reason must not exempt the delivery");
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: internal QA-only fixture correction".into();
        let refusal = park_refusal(
            cas_root.path(), repo.path(), &task, &["hub-web/src/app.ts".into()], &head, "main", None,
        );
        assert!(refusal.is_none(), "a typed decision with a reason should exempt the delivery: {refusal:?}");
    }

    #[test]
    fn changelog_gate_skips_projects_without_unreleased_section() {
        let repo = changelog_fixture();
        std::fs::remove_file(repo.path().join("CHANGELOG.md")).unwrap();
        let head = commit_changelog_fixture(repo.path(), "project without release notes");
        let task = Task::new("cas-changelog-fixture".into(), "delivery".into());

        assert!(changelog_refusal(
            repo.path(),
            &task,
            &["hub-web/src/app.ts".into()],
            &head,
            "main",
            None,
        ).is_none(), "projects without an Unreleased section are outside this gate");
    }

    #[test]
    fn changelog_templates_do_not_satisfy_entry_or_exception() {
        let repo = changelog_fixture();
        let mut task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        std::fs::write(repo.path().join("hub-web/src/app.ts"), "export const value = 2;\n").unwrap();
        let source_only = commit_changelog_fixture(repo.path(), "product change");
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: <reason>".into();
        let placeholder_reason = changelog_refusal(
            repo.path(),
            &task,
            &["hub-web/src/app.ts".into()],
            &source_only,
            "main",
            None,
        );

        task.notes.clear();
        let mut placeholder_entries_refused = true;
        for entry in [
            "### Added — <area>\n\n- **Commander shows session state** — The status line follows live events.",
            "### Added — Commander\n\n- **Title** — What changed.",
            "### Added — Commander\n\n- **Session state stays current** — <What changed>.",
        ] {
            let repo = changelog_fixture();
            std::fs::write(
                repo.path().join("hub-web/src/app.ts"),
                "export const value = 2;\n",
            ).unwrap();
            std::fs::write(
                repo.path().join("CHANGELOG.md"),
                format!(
                    "# Changelog\n\n## [Unreleased]\n\n{entry}\n\n## [1.0.0] - 2026-01-01\n"
                ),
            ).unwrap();
            let placeholder = commit_changelog_fixture(repo.path(), "add changelog template");
            let refusal = changelog_refusal(
                repo.path(),
                &task,
                &["CHANGELOG.md".into(), "hub-web/src/app.ts".into()],
                &placeholder,
                "main",
                None,
            );
            placeholder_entries_refused &= refusal.is_some();
        }
        assert!(
            placeholder_reason.is_some() && placeholder_entries_refused,
            "the literal templates must be refused: reason={placeholder_reason:?}; entries_refused={placeholder_entries_refused}"
        );
    }

    #[test]
    fn park_refuses_committed_root_handoff_cas_c55fa() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let task = Task::new("cas-c55fa".into(), "checkpoint delivery".into());
        std::fs::write(repo.path().join("HANDOFF.md"), "Resume the worker.\n").unwrap();
        let head = commit_changelog_fixture(repo.path(), "checkpoint");
        // Inspect the delivered commit, including a handoff retained from an
        // earlier push, rather than trusting the changed-path list or checkout.
        std::fs::remove_file(repo.path().join("HANDOFF.md")).unwrap();
        for changed in [vec!["HANDOFF.md".into()], vec!["docs/guide.md".into()]] {
            let refusals = park_refusals(cas_root.path(), repo.path(), &task, &changed, &head, "main", None);
            assert!(refusals.iter().any(|r| r.starts_with("ROOT_HANDOFF_COMMITTED:")), "{refusals:?}");
        }
        std::fs::create_dir_all(repo.path().join("docs")).unwrap();
        std::fs::write(repo.path().join("docs/HANDOFF.md"), "Project documentation.\n").unwrap();
        let clean = commit_changelog_fixture(repo.path(), "remove worker scratch notes");
        assert!(park_refusals(cas_root.path(), repo.path(), &task,
            &["HANDOFF.md".into(), "docs/HANDOFF.md".into()], &clean, "main", None).is_empty());
    }

    #[test]
    fn non_product_paths_do_not_require_a_changelog_entry() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let head = commit_changelog_fixture(repo.path(), "documentation update");
        let task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &["docs/guide.md".into()], &head, "main", None).is_none());
    }

    /// cas-d6eb AC2: a widening that adds only crate-qualified modules of
    /// crates tested at the parked head is covered by those receipts; one
    /// naming an untested crate, or no crate at all, is not.
    #[test]
    fn widening_covered_by_receipts_is_recognised_cas_d6eb() {
        let repo = workspace();
        let cas_root = tempfile::tempdir().unwrap();
        let head = "a".repeat(40);
        let added = |targets: &[&str]| targets.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            receipts_cover_targets(cas_root.path(), repo.path(), &head, &added(&["cas-cli::cli::factory"])),
            None,
            "no receipts at the head"
        );
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas", "test(worker)",
        );
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "code-mode-mcp", "test(worker)",
        );
        assert_eq!(
            receipts_cover_targets(
                cas_root.path(), repo.path(), &head,
                &added(&["cas-cli::cli::factory", "cas::lib", "cas-mcp-proxy::lib"]),
            ),
            Some(vec!["cas".to_string(), "code-mode-mcp".to_string()]),
        );
        for uncovered in [&["cas-store::prompt_queue_store"][..], &["factory"], &["cas-cli::cli::factory", "cas-store::x"]] {
            assert_eq!(
                receipts_cover_targets(cas_root.path(), repo.path(), &head, &added(uncovered)),
                None,
                "{uncovered:?}"
            );
        }
        assert_eq!(
            receipts_cover_targets(cas_root.path(), repo.path(), &"b".repeat(40), &added(&["cas::lib"])),
            None,
            "another head's receipts do not count"
        );
    }
}
