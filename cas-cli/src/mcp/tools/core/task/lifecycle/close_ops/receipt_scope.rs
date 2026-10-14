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
pub(super) fn for_close(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    delivered_tip: Option<&str>,
) -> Option<(Task, String)> {
    if !task.risk.contains(&TaskRisk::BlastRadius) {
        return None;
    }
    let head = delivered_tip?;
    let tested = crate::factory_worker_check::passing_test_packages(cas_root, proof_repo, head);
    let scope = derive(proof_repo, changed_paths, &task.proof_targets, &tested);
    (!scope.is_empty()).then(|| (scope.widen(task), scope.note(head)))
}

/// cas-d6eb: the refusal a blast-radius delivery gets at park, before parking
/// locks its proof scope, when it reaches modules that neither its declared
/// `proof_targets` nor a passing worker test receipt at `head` covers. The
/// close gate would refuse the same modules after verification, when only a
/// supervisor scope fix (and a fresh verification round) could repair them.
pub(super) fn park_refusal(
    cas_root: &Path,
    proof_repo: &Path,
    task: &Task,
    changed_paths: &[String],
    head: &str,
) -> Option<String> {
    if !task.risk.contains(&TaskRisk::BlastRadius) {
        return None;
    }
    let judged = for_close(cas_root, proof_repo, task, changed_paths, Some(head));
    let targets = judged.as_ref().map_or(&task.proof_targets, |(judged, _)| &judged.proof_targets);
    let uncovered = super::uncovered_blast_radius_modules(changed_paths, targets);
    if uncovered.is_empty() {
        return None;
    }
    let mut widened = cas_types::normalize_proof_targets(&task.proof_targets);
    for module in &uncovered {
        if !widened.contains(module) {
            widened.push(module.clone());
        }
    }
    let caller = crate::mcp::tools::core::guidance::caller_prefix();
    let short = &head[..head.len().min(12)];
    Some(format!(
        "⚠️ PROOF SCOPE INCOMPLETE (cas-d6eb): task {id} declares risk=blast-radius, and its delivery at {short} reaches source modules that neither its proof_targets nor a passing worker test receipt at that commit covers: {modules}. Parking locks the proof scope, so fix it now: run a capped targeted test in each module's crate at {short} (`cargo nextest run -p <crate> --lib -E 'test(<name>)'`), or widen the declaration with `{caller}task action=update id={id} proof_targets=\"{targets}\"`. Then retry `{caller}task action=close id={id}`.",
        id = task.id,
        modules = uncovered.join(", "),
        targets = widened.join(","),
    ))
}

/// cas-d6eb: the tested packages that cover every target a proof-scope
/// widening adds, when each target is crate-qualified (`<crate>::<path>`) and
/// its crate has a passing worker test receipt at `head`. Such a widening only
/// names what the close gate already accepts at that head ([`for_close`]), so
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
        let module = SourceModule { crate_dir, segments: rest.to_vec() };
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
    for module in changed_source_modules(changed_paths) {
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

/// Map each workspace package name to its crate directory's name, as
/// [`changed_source_modules`] records it (lowercase basename).
fn workspace_package_dirs(repo: &Path) -> BTreeMap<String, String> {
    let mut dirs = BTreeMap::new();
    let Some(root) = read_manifest(&repo.join("Cargo.toml")) else {
        return dirs;
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
    for path in manifests {
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

    /// cas-d6eb AC1: at park, a delivery reaching a crate nobody tested is
    /// told which modules to cover and how, while proof_targets can still be
    /// widened; once it widens them, or tests that crate, it parks.
    #[test]
    fn park_names_uncovered_modules_while_the_worker_can_fix_them_cas_d6eb() {
        let repo = workspace();
        let cas_root = tempfile::tempdir().unwrap();
        let head = "e".repeat(40);
        let mut task = blast_radius_task(&["close_ops"]);
        let changed = delivered();
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas", "test(worker)",
        );
        let refusal = park_refusal(cas_root.path(), repo.path(), &task, &changed, &head)
            .expect("an untested crate is refused at park");
        assert!(refusal.contains("PROOF SCOPE INCOMPLETE"), "{refusal}");
        // Only the untested crate's module; the tested crate is covered.
        assert!(refusal.contains("covers: prompt_queue_store::operator_cloud."), "{refusal}");
        assert!(
            refusal.contains(&format!(
                "task action=update id={} proof_targets=\"close_ops,prompt_queue_store::operator_cloud\"",
                task.id
            )),
            "{refusal}"
        );

        // Following the suggested update clears the refusal.
        task.proof_targets.push("prompt_queue_store::operator_cloud".into());
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &head).is_none());
        // So does a test receipt in the untested crate instead.
        task.proof_targets.pop();
        crate::factory_worker_check::record_passing_test_for_tests(
            cas_root.path(), repo.path(), &head, "cas-store", "test(worker)",
        );
        assert!(park_refusal(cas_root.path(), repo.path(), &task, &changed, &head).is_none());
        // A task without blast-radius risk is never refused here.
        let mut plain = blast_radius_task(&[]);
        plain.risk = vec![TaskRisk::None];
        assert!(park_refusal(cas_root.path(), repo.path(), &plain, &changed, &"f".repeat(40)).is_none());
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
