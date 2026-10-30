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
) -> Option<String> {
    let short = &head[..head.len().min(12)];
    if let Some(refusal) = changelog_refusal(proof_repo, task, changed_paths, head, target) {
        return Some(refusal);
    }
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    let tested = crate::factory_worker_check::passing_test_packages(cas_root, proof_repo, head);
    let untested = untested_changed_crates(proof_repo, changed_paths, &tested);
    if !untested.is_empty() {
        return Some(format!(
            "⚠️ FOCUSED PROOF REQUIRED (cas-eac2): task {id} changes Rust in crates that have no passing worker test receipt at {short}: {crates}. Lane CI no longer runs broad Rust tests for factory pushes, so the targeted fallback is yours. At {short}, run a capped targeted test for the changed behaviour in each crate (`cargo nextest run -p <crate> --lib -E 'test(<name>)'`, or `--test <harness>`), then retry `{caller}task action=close id={id}`. A receipt from another crate does not count.",
            id = task.id,
            crates = untested.join(", "),
        ));
    }
    // A workspace may live below the repository root; any changed Rust file
    // inside a Cargo package turns the touched-test check on.
    if changed_paths.iter().any(|path| path.ends_with(".rs")
        && Path::new(path).ancestors().skip(1).any(|dir| proof_repo.join(dir).join("Cargo.toml").is_file()))
    {
        let evidence = crate::factory_worker_check::PassingTests::new(cas_root, proof_repo, head);
        let refusal = match evidence.touched(target) {
            Ok(touched) => touched_test_refusal(task, &touched),
            Err(error) => Some(format!("selection could not be resolved: {error:#}")),
        };
        if let Some(reason) = refusal {
            return Some(format!(
                "TOUCHED TEST EVIDENCE REQUIRED at {short}.\n{reason}\nEvery test earns its place (cas-tdd): run the named tests through the capped worker lane, then retry `{caller}task action=close id={}`. Park only reads their bound receipts.", task.id
            ));
        }
    }
    // cas-d6eb: a blast-radius module neither declared nor covered by a
    // receipt, while proof_targets can still be widened (parking locks them).
    if !task.risk.contains(&TaskRisk::BlastRadius) {
        return None;
    }
    let judged = for_tested_packages(proof_repo, task, changed_paths, head, &tested);
    let targets = judged.as_ref().map_or(&task.proof_targets, |(judged, _)| &judged.proof_targets);
    let uncovered = super::uncovered_blast_radius_modules(proof_repo, changed_paths, targets);
    if uncovered.is_empty() {
        return None;
    }
    let mut widened = cas_types::normalize_proof_targets(&task.proof_targets);
    for module in &uncovered {
        if !widened.contains(module) {
            widened.push(module.clone());
        }
    }
    Some(format!(
        "⚠️ PROOF SCOPE INCOMPLETE (cas-d6eb): task {id} declares risk=blast-radius, and its delivery at {short} reaches source modules that neither its proof_targets nor a passing worker test receipt at that commit covers: {modules}. Parking locks the proof scope, so fix it now: run a capped targeted test in each module's crate at {short} (`cargo nextest run -p <crate> --lib -E 'test(<name>)'`), or widen the declaration with `{caller}task action=update id={id} proof_targets=\"{targets}\"`. Then retry `{caller}task action=close id={id}`.",
        id = task.id,
        modules = uncovered.join(", "),
        targets = widened.join(","),
    ))
}

/// A recorded `test-waiver: <kind> <test id> - <reason>` task note line.
fn waived(task: &Task, kind: &str, id: &str) -> bool {
    let marker = format!("test-waiver: {kind} {id} - ");
    task.notes.lines().any(|line| line.split_once(&marker).is_some_and(|(_, reason)| !reason.trim().is_empty()))
}

/// The touched-test rules park reads from receipts: five concurrent passes,
/// a failing run before the change, and the runtime budget.
fn touched_test_refusal(task: &Task, touched: &crate::factory_worker_check::Touched) -> Option<String> {
    let mut reasons = Vec::new();
    if !touched.unstable.is_empty() {
        reasons.push(format!(
            "missing five-run evidence for {}; five concurrent zero-retry passes are required.",
            touched.unstable.join(", ")
        ));
    }
    let characterization = task.execution_note.as_deref() == Some("characterization-first");
    let never_red = touched.never_red.iter()
        .filter(|id| !characterization && !waived(task, "fail-first", id))
        .map(String::as_str).collect::<Vec<_>>();
    if !never_red.is_empty() {
        reasons.push(format!(
            "no failing run before the change for {}. Run the test red on a commit before the fix, or revert the fix uncommitted and run `CAS_RED_CONTROL=<label> cargo nextest run ...` (snapshotted to refs/cas/red, never pushed). A test that never failed proves nothing; if it truly cannot fail first, note `test-waiver: fail-first <test id> - <reason>`.",
            never_red.join(", ")
        ));
    }
    let over = touched.over_budget.iter()
        .filter(|(id, _)| !waived(task, "budget", id))
        .map(|(id, reason)| format!("{id} ({reason})")).collect::<Vec<_>>();
    if !over.is_empty() {
        reasons.push(format!(
            "over the runtime budget (unit under 1s, integration under 10s): {}. Move it to a cheaper layer or speed it up; a justified exception notes `test-waiver: budget <test id> - <reason>`.",
            over.join(", ")
        ));
    }
    (!reasons.is_empty()).then(|| reasons.join("\n"))
}

/// cas-eac2 (D2): the refusal for a tip of a repository that ships
/// `scripts/fast-rows-pre-push.sh` when no `fast-rows: PASS <tip>` receipt
/// exists. That happens when the push skipped the hook through
/// `CAS_SKIP_FAST_ROWS` or `--no-verify`, or when the rows failed.
fn fast_rows_refusal(repo: &Path, task_id: &str, head: &str, target: &str) -> Option<String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|out| out.status.success())
    };
    git(&["cat-file", "-e", &format!("{head}:scripts/fast-rows-pre-push.sh")])?;
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let receipt = Path::new(String::from_utf8_lossy(&common.stdout).trim())
        .join("cas/fast-rows")
        .join(format!("{head}.pass"));
    let text = std::fs::read_to_string(&receipt).unwrap_or_default();
    if text.starts_with(&format!("fast-rows: PASS {head} ")) {
        return None;
    }
    let fixes: String = text
        .lines()
        .filter(|line| line.starts_with("FIX "))
        .take(10)
        .map(|line| format!("\n- {line}"))
        .collect();
    Some(format!(
        "⚠️ FAST ROWS REQUIRED (cas-eac2): task {task_id}'s tip {short} has no fast-rows PASS receipt ({path}). Lane CI no longer runs these no-build rows for factory pushes; your pre-push hook runs them (scripts/fast-rows-pre-push.sh), and park only reads the receipt.{fixes}\nWith everything committed at {short}, run `./scripts/release-gate.sh --fast-rows --base origin/{target}` (the hook's command), apply each FIX, push, then retry close.",
        short = &head[..head.len().min(12)],
        path = receipt.display(),
        target = target.strip_prefix("origin/").unwrap_or(target),
    ))
}

fn product_source_path(path: &str) -> bool {
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
) -> Option<String> {
    if !changed_paths.iter().any(|path| product_source_path(path))
        || has_no_changelog_decision(&task.notes)
        || !has_unreleased_section_at_head(repo, head)
    {
        return None;
    }
    let changelog_changed = changed_paths.iter().any(|path| path == "CHANGELOG.md");
    if changelog_changed && has_titled_unreleased_delta(repo, head, target) {
        return None;
    }
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    Some(format!(
        "⚠️ CHANGELOG [Unreleased] REQUIRED (cas-a945): task {} changes shipped product or developer source but has no new or extended titled entry under `CHANGELOG.md`'s `## [Unreleased]` section. Add a per-delivery line such as `- **Title** — What changed.` under a `### Added — <area>` or `### Fixed — <area>` heading, then retry `{caller}task action=close id={}`. For test-only, docs-only or internal QA work, record `{caller}task action=notes id={} note_type=decision notes=\"no changelog: <reason>\"` with a specific reason.",
        task.id, task.id, task.id,
    ))
}

fn has_no_changelog_decision(notes: &str) -> bool {
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
        body.strip_prefix("no changelog:").is_some_and(|reason| {
            let reason = reason.trim();
            !reason.is_empty() && !is_changelog_template_value(reason)
        })
    })
}

fn has_unreleased_section_at_head(repo: &Path, head: &str) -> bool {
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

fn has_titled_unreleased_delta(repo: &Path, head: &str, target: &str) -> bool {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let Some(base) = git(&["merge-base", head, target]) else {
        return false;
    };
    let range = format!("{}..{head}", base.trim());
    let Some(commits) = git(&["rev-list", "--first-parent", "--reverse", &range]) else {
        return false;
    };
    let Some(final_text) = git(&["show", &format!("{head}:CHANGELOG.md")]) else {
        return false;
    };
    let Some(final_entries) = titled_unreleased_entries(&final_text) else {
        return false;
    };

    for commit in commits.lines().filter(|commit| !commit.is_empty()) {
        let Some(parents) = git(&["rev-list", "--parents", "-n", "1", commit]) else {
            continue;
        };
        let parents: Vec<_> = parents.split_whitespace().collect();
        let [_, parent] = parents.as_slice() else {
            continue;
        };
        let Some(changed) = git(&["diff", "--name-only", parent, commit, "--", "CHANGELOG.md"])
        else {
            continue;
        };
        if !changed.lines().any(|path| path == "CHANGELOG.md") {
            continue;
        }
        let Some(previous) = git(&["show", &format!("{parent}:CHANGELOG.md")]) else {
            continue;
        };
        let Some(current) = git(&["show", &format!("{commit}:CHANGELOG.md")]) else {
            continue;
        };
        let old_entries = titled_unreleased_entries(&previous).unwrap_or_default();
        let Some(new_entries) = titled_unreleased_entries(&current) else {
            continue;
        };
        if new_entries.iter().any(|(key, entry)| {
            old_entries.get(key) != Some(entry) && final_entries.get(key) == Some(entry)
        }) {
            return true;
        }
    }
    false
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
fn workspace_package_dirs(repo: &Path) -> BTreeMap<String, String> {
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
        let git = |args: &[&str]| {
            let output = crate::test_env_guard::git_fixture(repo.path()).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let cas_root = tempfile::tempdir().unwrap();
        let head = git(&["rev-parse", "HEAD"]);
        let mut task = blast_radius_task(&["close_ops"]);
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: test-only receipt-gate coverage".into();
        let changed = delivered();
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas", "test(worker)",
        );
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &head, "main")
            .expect("an untested crate is refused at park");
        assert!(refusal.contains("FOCUSED PROOF REQUIRED"), "{refusal}");
        assert!(refusal.contains(&format!("at {}: cas-store.", &head[..12])), "{refusal}");

        task.proof_targets.push("prompt_queue_store::operator_cloud".into());
        let mut plain = task.clone();
        plain.risk = vec![TaskRisk::None];
        for task in [&task, &plain] {
            assert!(park_refusal(cas_root.path(), repo.path(), task, &changed, &head, "main").is_some());
        }
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas-store", "test(worker)",
        );
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &head, "main").is_none());

        git(&["switch", "-qc", "factory/touched"]);
        std::fs::create_dir_all(repo.path().join("cas-cli/src")).unwrap();
        std::fs::write(repo.path().join("cas-cli/src/lib.rs"), "#[test] fn new_test() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "new test"]);
        let red = git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.path().join("cas-cli/src/fix.rs"), "pub fn fixed() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "fix"]);
        let delivery = git(&["rev-parse", "HEAD"]);
        for package in ["cas", "cas-store"] {
            crate::factory_worker_check::record_passing_test_for_tests(
                cas_root.path(), repo.path(), &delivery, package, "test(worker)",
            );
        }
        let id = "cas-cli/src/lib.rs::new_test";
        let refusal = park_refusal(cas_root.path(), repo.path(), &plain, &changed, &delivery, "main").unwrap();
        assert!(refusal.contains("TOUCHED TEST EVIDENCE REQUIRED"), "{refusal}");
        assert!(refusal.contains(&format!("missing five-run evidence for {id}")), "{refusal}");
        // Each touched-test rule refuses alone and is met by its own receipt or waiver.
        crate::factory_worker_check::record_stability_with_durations_for_tests(
            cas_root.path(), repo.path(), &delivery, &head, "cas", "test(worker)", id, 2.5,
        );
        let refusal = park_refusal(cas_root.path(), repo.path(), &plain, &changed, &delivery, "main").unwrap();
        assert!(!refusal.contains("five-run"), "{refusal}");
        assert!(refusal.contains(&format!("no failing run before the change for {id}")), "{refusal}");
        assert!(refusal.contains(&format!("{id} (median 2.500s over the 1s budget)")), "{refusal}");
        let mut characterization = plain.clone();
        characterization.execution_note = Some("characterization-first".into());
        let refusal = park_refusal(cas_root.path(), repo.path(), &characterization, &changed, &delivery, "main").unwrap();
        assert!(!refusal.contains("no failing run"), "characterization-first pins current behaviour: {refusal}");
        crate::factory_worker_check::record_red_for_tests(cas_root.path(), repo.path(), &red, &delivery, "main", id);
        let refusal = park_refusal(cas_root.path(), repo.path(), &plain, &changed, &delivery, "main").unwrap();
        assert!(!refusal.contains("no failing run"), "{refusal}");
        plain.notes = format!("[2026-10-29 21:00] note test-waiver: budget {id} - spawns a real git fixture");
        assert!(park_refusal(cas_root.path(), repo.path(), &plain, &changed, &delivery, "main").is_none());
        crate::factory_worker_check::record_stability_for_tests(
            cas_root.path(), repo.path(), &delivery, &head, "cas", "test(worker)", id,
        );
        plain.notes.clear();
        assert!(park_refusal(cas_root.path(), repo.path(), &plain, &changed, &delivery, "main").is_none());

        // The touched-test check also finds a workspace below the repository root.
        let nested = tempfile::tempdir().unwrap();
        let nested_git = |args: &[&str]| {
            let output = crate::test_env_guard::git_fixture(nested.path()).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        std::fs::create_dir_all(nested.path().join("rust/src")).unwrap();
        std::fs::write(nested.path().join("rust/Cargo.toml"), "[package]\nname = \"nested\"\n").unwrap();
        nested_git(&["init", "-q", "-b", "main"]);
        nested_git(&["add", "."]);
        nested_git(&["commit", "-qm", "base"]);
        nested_git(&["switch", "-qc", "factory/nested"]);
        std::fs::write(nested.path().join("rust/src/lib.rs"), "#[test] fn nested_test() {}\n").unwrap();
        nested_git(&["add", "."]);
        nested_git(&["commit", "-qm", "nested test"]);
        let tip = nested_git(&["rev-parse", "HEAD"]);
        let refusal = park_refusal(cas_root.path(), nested.path(), &plain, &["rust/src/lib.rs".into()], &tip, "main")
            .expect("a subdirectory workspace still needs five-run evidence");
        assert!(refusal.contains("rust/src/lib.rs::nested_test"), "{refusal}");

        let bare = tempfile::tempdir().unwrap();
        let mut bare_task = blast_radius_task(&[]);
        bare_task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: test-only proof-scope fixture".into();
        let scope = park_refusal(cas_root.path(), bare.path(), &bare_task, &changed, &head, "main")
            .expect("undeclared modules outside a workspace");
        assert!(scope.contains("PROOF SCOPE INCOMPLETE (cas-d6eb)"), "{scope}");
    }

    #[cfg(unix)]
    #[test]
    fn nested_workspace_capped_stability_is_accepted_at_park() {
        use std::os::unix::fs::PermissionsExt;
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("CAS_FACTORY_DISABLE_TARGET_SEED", "1");
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".cas");
        let repo = root.join("worktrees/nested");
        std::fs::create_dir_all(repo.join("src-tauri/widget")).unwrap();
        let git = |args: &[&str]| {
            let output = crate::test_env_guard::git_fixture(&repo).args(args).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(repo.join("src-tauri/Cargo.toml"), "[workspace]\nmembers=[\"widget\"]\n").unwrap();
        std::fs::write(repo.join("src-tauri/widget/Cargo.toml"), "[package]\nname=\"nested\"\nversion=\"0.1.0\"\n[[test]]\nname=\"fixture\"\npath=\"checks.rs\"\n").unwrap();
        let source = repo.join("src-tauri/widget/checks.rs");
        std::fs::write(&source, "fn fixed() -> bool { false }\n").unwrap();
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]); git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("config.toml"), "[factory]\nepic_base_branch=\"main\"\n").unwrap();
        git(&["switch", "-qc", "factory/nested"]);
        std::fs::write(&source, "fn fixed() -> bool { false }\n#[test] fn nested_test() { assert!(fixed()); }\n").unwrap();
        git(&["add", "."]); git(&["commit", "-qm", "test"]);
        let cargo = dir.path().join("cargo-fixture");
        // Mock only the external Cargo boundary. Five trial children rendezvous;
        // real admission, selection, receipt storage and park validation run.
        std::fs::write(&cargo, r#"#!/usr/bin/env python3
import fcntl,json,os,sys,time
from pathlib import Path
args=sys.argv[1:]
assert '--manifest-path' in args, args
manifest=Path(args[args.index('--manifest-path')+1])
assert manifest.resolve()==Path('src-tauri/widget/Cargo.toml').resolve(),manifest
if args[:2]==['nextest','list']:
 print(json.dumps({'rust-suites':{'fixture':{'testcases':{'nested_test':{'ignored':False}}}}}))
 sys.exit(0)
failed='false' in Path('src-tauri/widget/checks.rs').read_text()
if '--retries' in args:
 state=Path('target/rendezvous')
 with state.open('a+') as file:
  fcntl.flock(file,fcntl.LOCK_EX)
  file.write('ready\n');file.flush()
 deadline=time.monotonic()+10
 while len(state.read_text().splitlines())<5:
  assert time.monotonic()<deadline,'trial children did not rendezvous'
  time.sleep(.001)
print(f'        {"FAIL" if failed else "PASS"} [   0.010s] (1/1) nested::fixture nested_test')
print('     Summary [ 0.01s] 1 test run: '+('0 passed, 1 failed' if failed else '1 passed, 0 skipped'))
sys.exit(100 if failed else 0)
"#).unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = ["nextest", "run", "-p", "nested", "--test", "fixture", "-E", "test(nested_test)"]
            .map(str::to_owned);
        let red = crate::factory_worker_check::execute_at(&root, &args, &repo, &cargo);
        assert!(red.is_err(), "the unfixed fixture must fail");
        std::fs::write(&source, "fn fixed() -> bool { true }\n#[test] fn nested_test() { assert!(fixed()); }\n").unwrap();
        git(&["add", "."]); git(&["commit", "-qm", "fix"]);
        let head = git(&["rev-parse", "HEAD"]);
        crate::factory_worker_check::execute_at(&root, &args, &repo, &cargo)
            .expect("a subdirectory-only workspace must produce a capped stability receipt");
        let mut task = blast_radius_task(&[]);
        task.risk = vec![TaskRisk::None];
        task.notes = "[2026-10-29 13:00] DECISION no changelog: test-only fixture".into();
        let refusal = park_refusal(&root, &repo, &task, &["src-tauri/widget/checks.rs".into()], &head, "main");
        assert!(refusal.is_none(), "capped evidence must park without an override: {refusal:?}");

        // Reuse this fixture to exercise the standalone Python fallback without
        // runtime target metadata; it must discover the same nearest manifest.
        std::fs::remove_file(repo.join("target/rendezvous")).unwrap();
        let output = std::process::Command::new("python3")
            .args(["-I", "-B", "-c"])
            .arg(concat!(
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/rust_test_source.py")), "\n",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/worker-test-stability.py")),
            ))
            .args(["--base", &base, "--cargo"]).arg(&cargo)
            .arg("--verified-script").arg(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/run-verified-tests.sh")))
            .arg("--count-file").arg(repo.join("target/python-count"))
            .arg("--").args(&args).current_dir(&repo)
            .env_remove("CAS_WORKER_STABILITY_TARGET")
            .env("VERIFIED_TEST_REPO_ROOT", &repo).output().unwrap();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let proof: serde_json::Value = serde_json::from_slice(&std::fs::read(repo.join("target/python-count.stability.json")).unwrap()).unwrap();
        assert_eq!(proof["tests"], serde_json::json!(["src-tauri/widget/checks.rs::nested_test"]));
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
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &source_only, "main")
            .expect("product source without a changelog entry must be refused");
        assert!(refusal.contains("CHANGELOG [Unreleased] REQUIRED"), "{refusal}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- Generic status entry.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let untitled = commit_changelog_fixture(repo.path(), "add an untitled note");
        let changed = vec!["CHANGELOG.md".to_string(), "hub-web/src/app.ts".to_string()];
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &untitled, "main")
            .expect("an untitled Unreleased bullet is not release evidence");
        assert!(refusal.contains("CHANGELOG [Unreleased] REQUIRED"), "{refusal}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- **Status updates stay current** — Status now reflects the latest response.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let titled = commit_changelog_fixture(repo.path(), "add release note");
        let text = std::fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        assert_eq!(titled_unreleased_entries(&text).unwrap().len(), 1);
        assert!(has_titled_unreleased_delta(repo.path(), &titled, "main"));
        let accepted = park_refusal(cas_root.path(), repo.path(), &task, &changed, &titled, "main");
        assert!(accepted.is_none(), "{accepted:?}");

        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Fixed — Commander\n\n- **Status updates stay current** — Status now follows the current response.\n\n## [1.0.0] - 2026-01-01\n",
        )
        .unwrap();
        let extended = commit_changelog_fixture(repo.path(), "extend release note");
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &extended, "main").is_none());
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
        )
        .is_some(), "an empty reason must not exempt the delivery");
        task.notes = "[2026-10-29 13:00] ✅ DECISION no changelog: internal QA-only fixture correction".into();
        let refusal = park_refusal(
            cas_root.path(), repo.path(), &task, &["hub-web/src/app.ts".into()], &head, "main",
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
            );
            placeholder_entries_refused &= refusal.is_some();
        }
        assert!(
            placeholder_reason.is_some() && placeholder_entries_refused,
            "the literal templates must be refused: reason={placeholder_reason:?}; entries_refused={placeholder_entries_refused}"
        );
    }

    #[test]
    fn non_product_paths_do_not_require_a_changelog_entry() {
        let repo = changelog_fixture();
        let cas_root = tempfile::tempdir().unwrap();
        let head = commit_changelog_fixture(repo.path(), "documentation update");
        let task = Task::new("cas-changelog-fixture".into(), "delivery".into());
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &["docs/guide.md".into()], &head, "main").is_none());
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
