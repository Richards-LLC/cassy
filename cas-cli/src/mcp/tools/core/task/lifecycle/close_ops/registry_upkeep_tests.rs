use super::*;
use crate::config::RegistryEnforcement;
use cas_store::TaskStore;
use std::process::Command;

const YAML: &str = "version: 1\nupdated: '2026-10-21'\nsurfaces: ['src/pages/**']\ninvariants:\n  INV: {statement: Arrived, check: Read state}\njourneys:\n  - id: signup\n    status: live\n    seeded: true\n    entry: {url: /signup, trigger: Open}\n    owners: ['src/pages/signup.ts']\n";
struct Fixture {
    repo: tempfile::TempDir,
    _db: tempfile::TempDir,
    store: cas_store::SqliteTaskStore,
    task: Task,
    qa: QaConfig,
    base: String,
}
fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "upkeep fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "upkeep fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}
fn commit(repo: &Path) -> String {
    git(repo, &["add", "."]);
    git(
        repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    );
    git(repo, &["rev-parse", "HEAD"])
}
impl Fixture {
    fn new(yaml: &str) -> Self {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        write(repo.path(), "src/pages/signup.ts", "signup");
        write(
            repo.path(),
            ".claude/CODEMAP.md",
            "- src/pages — Account entry paths\n",
        );
        write(repo.path(), crate::qa_registry::DEFAULT_PATH, yaml);
        let base = commit(repo.path());
        let db = tempfile::tempdir().unwrap();
        let store = cas_store::SqliteTaskStore::open(db.path()).unwrap();
        store.init().unwrap();
        let mut task = Task::new("cas-1234".into(), "Entry change".into());
        task.task_type = TaskType::Feature;
        store.add(&task).unwrap();
        Self {
            repo,
            _db: db,
            store,
            task,
            qa: QaConfig::default(),
            base,
        }
    }
    fn run(&mut self, head: &str) -> Result<Vec<String>, String> {
        let mut config = crate::config::Config::default();
        config.qa = Some(self.qa.clone());
        config.save(self._db.path()).unwrap();
        at_park(
            &mut CloseNoteTracker::new(&self.store, &self.task),
            &mut self.task,
            self._db.path(),
            self.repo.path(),
            &self.base,
            None,
            Some(head),
            None,
        )
    }
    fn decision(&mut self, body: &str) {
        append_close_decision_note(&self.store, &mut self.task, &format!("✅ DECISION {body}"));
    }
    fn duplicate_family() -> Self {
        let mut f = Self::new(&format!(
            "{YAML}bug_families:\n  duplicate: {{title: Alternate case, tasks: [cas-5678], invariants: [INV], journeys: [signup]}}\n"
        ));
        write(
            f.repo.path(),
            "docs/qa/bug-families/duplicate.yaml",
            "title: Existing case\ntasks: [cas-1234]\ninvariants: [INV]\njourneys: [signup]\n",
        );
        f.base = commit(f.repo.path());
        f.task.task_type = TaskType::Bug;
        f.task.updated_at = f.store.update(&f.task).unwrap();
        f
    }
}

#[test]
fn registry_upkeep_receipt_uses_target_ownership_for_delivery_files() {
    for owner_added in [false, true] {
        let mut f = Fixture::new(YAML);
        write(f.repo.path(), "src/pages/email.ts", "delivered entry");
        let receipt = commit(f.repo.path());
        // The target advances after the delivery, including an unrelated surface
        // that must never become part of this task's ownership check.
        write(f.repo.path(), "src/pages/unrelated.ts", "another task");
        if owner_added {
            write(
                f.repo.path(),
                crate::qa_registry::DEFAULT_PATH,
                &YAML.replace(
                    "owners: ['src/pages/signup.ts']",
                    "owners: ['src/pages/signup.ts', 'src/pages/email.ts']",
                ),
            );
        }
        commit(f.repo.path());
        // A different checkout cannot supply an unlanded ownership escape.
        git(f.repo.path(), &["checkout", "-qb", "other-work"]);
        write(
            f.repo.path(),
            crate::qa_registry::DEFAULT_PATH,
            &format!("{YAML}ignore: ['src/pages/**']\n"),
        );
        commit(f.repo.path());
        let mut config = crate::config::Config::default();
        config.qa = Some(f.qa.clone());
        config.save(f._db.path()).unwrap();
        let result = at_park(
            &mut CloseNoteTracker::new(&f.store, &f.task),
            &mut f.task,
            f._db.path(),
            f.repo.path(),
            "main",
            None,
            Some(&receipt),
            None,
        );
        if owner_added {
            assert!(
                result.is_ok(),
                "target ownership repair must allow close: {result:?}"
            );
        } else {
            let error = result.unwrap_err();
            assert!(
                error.contains("UNOWNED USER-FACING FILE")
                    && error.contains("src/pages/email.ts")
                    && !error.contains("src/pages/unrelated.ts"),
                "{error}"
            );
        }
    }
}

#[test]
fn registry_upkeep_added_surface_requires_ownership_or_reasoned_escape() {
    let mut f = Fixture::new(YAML);
    write(f.repo.path(), "src/pages/email.ts", "new entry");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("UNOWNED USER-FACING FILE")
            && error.contains("src/pages/email.ts")
            && error.contains("Account entry paths"),
        "{error}"
    );
    append_close_decision_note(
        &f.store,
        &mut f.task,
        "📝 PROGRESS quoted [2026-10-21 00:00] ✅ DECISION no journey impact: example only",
    );
    assert!(f.run(&head).is_err(), "quoted progress cannot waive a gate");
    f.decision("no journey impact:");
    assert!(f.run(&head).is_err(), "empty escape reason must refuse");
    f.decision("no journey impact: generated redirect uses signup");
    f.run(&head).unwrap();
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("UPKEEP ESCAPE G2")
    );
    let mut epic = Task::new("cas-epic".into(), "Release".into());
    epic.task_type = TaskType::Epic;
    f.store.add(&epic).unwrap();
    f.store
        .add_dependency(&cas_types::Dependency::new(
            f.task.id.clone(),
            epic.id.clone(),
            cas_types::DependencyType::ParentChild,
        ))
        .unwrap();
    let summary = epic_escape_summary(&f.store, &epic);
    assert!(
        summary.contains("Upkeep escapes in this epic: 1") && summary.contains("cas-1234: G2"),
        "{summary}"
    );
    let mut f = Fixture::new(YAML);
    write(f.repo.path(), "src/pages/legacy.ts", "old");
    f.base = commit(f.repo.path());
    write(f.repo.path(), "src/pages/legacy.ts", "changed");
    let head = commit(f.repo.path());
    f.run(&head).unwrap();
    f.qa.registry.enforce = RegistryEnforcement::Strict;
    assert!(f.run(&head).unwrap_err().contains("src/pages/legacy.ts"));
    at_park(
        &mut CloseNoteTracker::new(&f.store, &f.task),
        &mut f.task,
        f._db.path(),
        f.repo.path(),
        &f.base,
        None,
        Some(&head),
        Some("reviewed legacy copy correction"),
    )
    .unwrap();
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("supervisor override")
    );
    for suffix in [
        "ignore: ['src/pages/new.ts']\n",
        "owner_groups:\n  extra: {description: New, globs: ['src/pages/new.ts']}\n",
    ] {
        let mut f = Fixture::new(YAML);
        write(f.repo.path(), "src/pages/new.ts", "entry");
        let updated = if suffix.starts_with("owner_groups") {
            YAML.replace("    owners:", "    owner_groups: [extra]\n    owners:")
        } else {
            YAML.into()
        };
        write(
            f.repo.path(),
            crate::qa_registry::DEFAULT_PATH,
            &format!("{updated}{suffix}"),
        );
        let head = commit(f.repo.path());
        f.run(&head).unwrap();
    }
}

#[test]
fn registry_upkeep_rename_orphans_refuse_but_existing_debt_warns() {
    // A failed selector must not erase the G1 findings already computed.
    let mut failed_impact = Fixture::new(&format!("selection: legacy_catalog\n{YAML}"));
    git(
        failed_impact.repo.path(),
        &["mv", "src/pages/signup.ts", "src/pages/renamed.ts"],
    );
    let head = commit(failed_impact.repo.path());
    let error = failed_impact.run(&head).unwrap_err();
    assert!(
        error.contains("JOURNEY REGISTRY BROKEN")
            && error.contains("signup.ts")
            && error.contains("legacy_catalog selection requires docs/qa/journeys.md"),
        "{error}"
    );
    failed_impact.decision("registry unchanged: journey retirement reviewed");
    assert!(
        failed_impact
            .run(&head)
            .unwrap()
            .join("\n")
            .contains("not blocking"),
        "unavailable analysis alone cannot establish a diff-caused problem"
    );

    let yaml = YAML.replace(
        "surfaces:",
        "owner_groups:\n  historical: {description: Old, globs: ['old/**']}\nsurfaces:",
    );
    let mut f = Fixture::new(&yaml);
    git(
        f.repo.path(),
        &["mv", "src/pages/signup.ts", "src/pages/renamed.ts"],
    );
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("JOURNEY REGISTRY BROKEN")
            && error.contains("signup.ts")
            && error.contains("not blocking")
            && error.contains("old/**"),
        "{error}"
    );
    f.decision("registry unchanged: historical journey retired separately");
    f.decision("no journey impact: rename only");
    f.run(&head).unwrap();
    let mut f = Fixture::new(&yaml);
    write(f.repo.path(), "README.md", "documentation");
    let head = commit(f.repo.path());
    assert!(f.run(&head).unwrap().join("\n").contains("not blocking"));
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("not blocking")
    );
    let mut f = Fixture::new(YAML);
    git(
        f.repo.path(),
        &["mv", "src/pages/signup.ts", "src/pages/renamed.ts"],
    );
    write(f.repo.path(), "README.md", "mine");
    let head = commit(f.repo.path());
    check(
        &mut CloseNoteTracker::new(&f.store, &f.task),
        &mut f.task,
        f.repo.path(),
        &f.qa,
        &f.base,
        &head,
        &head,
        Some(&["README.md".into()]),
        None,
    )
    .unwrap();
}

#[test]
fn registry_upkeep_parallel_bug_families_merge_and_satisfy_case_gate() {
    let mut f = Fixture::new(YAML);
    f.task.task_type = TaskType::Bug;
    f.task.updated_at = f.store.update(&f.task).unwrap();
    let mut deliveries = Vec::new();
    for (branch, task) in [("alpha", "cas-1234"), ("beta", "cas-5678")] {
        git(f.repo.path(), &["checkout", "-qb", branch, &f.base]);
        write(
            f.repo.path(),
            &format!("docs/qa/bug-families/{branch}.yaml"),
            &format!("title: {branch}\ntasks: [{task}]\ninvariants: [INV]\njourneys: [signup]\n"),
        );
        deliveries.push(commit(f.repo.path()));
    }
    git(f.repo.path(), &["merge", "--no-edit", "alpha"]);
    for delivery in deliveries {
        let presence = delivery_content_presence_on_target(f.repo.path(), &delivery, "HEAD");
        assert!(
            matches!(presence, DeliveryContentPresence::Present { .. }),
            "{presence:?}"
        );
    }
    let registry =
        crate::qa_registry::load(&f.repo.path().join(crate::qa_registry::DEFAULT_PATH)).unwrap();
    assert!(registry.bug_families.contains_key("alpha"));
    assert!(registry.bug_families.contains_key("beta"));
    assert!(
        registry
            .validate(&tracked(f.repo.path(), "HEAD").unwrap())
            .problems
            .is_empty()
    );
    write(f.repo.path(), "src/pages/signup.ts", "fixed");
    let head = commit(f.repo.path());
    // Dirty checkout data must never replace the delivered snapshot's cases.
    write(
        f.repo.path(),
        "docs/qa/bug-families/alpha.yaml",
        "invalid: [",
    );
    for id in ["cas-1234", "cas-5678"] {
        f.task.id = id.into();
        let (findings, _) =
            inspect(&f.task, f.repo.path(), &f.qa, &f.base, &head, &head, None).unwrap();
        assert!(
            findings.is_empty(),
            "{id}: {}",
            findings
                .iter()
                .map(|f| f.detail.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    f.base = head;
    f.task.id = "cas-1234".into();
    write(
        f.repo.path(),
        "docs/qa/bug-families/alpha.yaml",
        "title: alpha\ntasks: [cas-1234]\ninvariants: [INV]\njourneys: [signup]\n",
    );
    write(f.repo.path(), "src/pages/signup.ts", "another fix");
    let head = commit(f.repo.path());
    assert!(f.run(&head).unwrap_err().contains("BUG FIX WITHOUT A CASE"));
}

#[test]
fn registry_upkeep_repaired_base_duplicate_evaluates_ownership_and_changed_case() {
    let mut f = Fixture::duplicate_family();
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, YAML);
    write(f.repo.path(), "src/pages/signup.ts", "another fix");
    write(f.repo.path(), "src/pages/email.ts", "new entry");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("base registry duplicate repaired")
            && error.contains("UNOWNED USER-FACING FILE")
            && error.contains("BUG FIX WITHOUT A CASE"),
        "{error}"
    );
    // Adding an actual new case and owning the new file must satisfy both gates.
    let repaired = YAML
        .replace("check: Read state", "check: Read state, bug: {task: cas-1234}")
        .replace("'src/pages/signup.ts'", "'src/pages/signup.ts', 'src/pages/email.ts'");
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, &repaired);
    let head = commit(f.repo.path());
    assert!(f.run(&head).unwrap().join("\n").contains("base registry duplicate repaired"));
}

#[test]
fn registry_upkeep_registry_escape_cannot_skip_g3_after_duplicate_repair() {
    let mut f = Fixture::duplicate_family();
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, YAML);
    write(f.repo.path(), "src/pages/signup.ts", "another fix");
    let head = commit(f.repo.path());
    f.decision("registry unchanged: duplicate repair reviewed");
    let error = f.run(&head).expect_err("G1 escape cannot skip the unchanged-case gate");
    assert!(error.contains("BUG FIX WITHOUT A CASE"), "{error}");
    f.decision("no case: reviewed copy correction without behavior change");
    f.run(&head).expect("explicit G3 waiver remains available after evaluation");
}

#[test]
fn registry_upkeep_persistent_duplicate_refuses_even_with_registry_escape() {
    let mut f = Fixture::duplicate_family();
    write(f.repo.path(), "src/pages/signup.ts", "another fix");
    let head = commit(f.repo.path());
    let error = f.run(&head).expect_err("persistent duplicate must refuse, not warn");
    assert!(
        error.contains("DELIVERED JOURNEY REGISTRY INVALID")
            && error.contains("duplicate bug family \"duplicate\"")
            && error.contains("docs/qa/bug-families/duplicate.yaml"),
        "{error}"
    );
    f.decision("registry unchanged: existing duplicate reviewed");
    assert!(f.run(&head).unwrap_err().contains("DELIVERED JOURNEY REGISTRY INVALID"));
}

#[test]
fn registry_upkeep_base_git_read_failure_warns_without_crediting_unchanged_case() {
    let yaml = format!("{YAML}bug_families:\n  unchanged: {{title: Existing case, tasks: [cas-1234], invariants: [INV], journeys: [signup]}}\n");
    let mut f = Fixture::new(&yaml);
    f.task.task_type = TaskType::Bug;
    f.task.updated_at = f.store.update(&f.task).unwrap();
    // Different blobs, identical case: the delivered registry remains readable.
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, &format!("# delivery\n{yaml}"));
    write(f.repo.path(), "src/pages/signup.ts", "another fix");
    let head = commit(f.repo.path());
    let blob = git(f.repo.path(), &["rev-parse", &format!("{}:{}", f.base, crate::qa_registry::DEFAULT_PATH)]);
    let object = f.repo.path().join(".git/objects").join(&blob[..2]).join(&blob[2..]);
    let bytes = std::fs::read(&object).unwrap();
    std::fs::remove_file(&object).unwrap();
    let result = check(
        &mut CloseNoteTracker::new(&f.store, &f.task),
        &mut f.task, f.repo.path(), &f.qa, &f.base, &head, &head, None, None,
    );
    // Restore the isolated fixture before asserting the normal case gate too.
    std::fs::write(&object, bytes).unwrap();
    let warnings = result.unwrap();
    assert!(warnings.iter().any(|text| text.contains("registry inputs unavailable")), "{warnings:?}");
    assert!(f.store.get(&f.task.id).unwrap().notes.contains("registry inputs unavailable"));
    assert!(f.run(&head).unwrap_err().contains("BUG FIX WITHOUT A CASE"));
}

#[test]
fn registry_upkeep_bug_requires_changed_case_for_task() {
    let mut f = Fixture::new(YAML);
    f.task.task_type = TaskType::Bug;
    f.task.updated_at = f.store.update(&f.task).unwrap();
    write(f.repo.path(), "src/pages/signup.ts", "fixed");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("BUG FIX WITHOUT A CASE")
            && error.contains("signup")
            && error.contains("cas-1234"),
        "{error}"
    );
    f.decision("no case: copy correction only");
    f.run(&head).unwrap();
    for case in [
        "bug_families:\n  claim: {title: Claim, tasks: [cas-1234], invariants: [INV], journeys: [signup]}\n",
        "invariants:\n  INV: {statement: Arrived, check: Read state, bug: {task: cas-1234}}\n",
    ] {
        let mut f = Fixture::new(YAML);
        f.task.task_type = TaskType::Bug;
        f.task.updated_at = f.store.update(&f.task).unwrap();
        write(f.repo.path(), "src/pages/signup.ts", "fixed");
        let updated = if case.starts_with("invariants:") {
            YAML.replace(
                "invariants:\n  INV: {statement: Arrived, check: Read state}\n",
                case,
            )
        } else {
            format!("{YAML}{case}")
        };
        write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, &updated);
        let head = commit(f.repo.path());
        f.run(&head).unwrap();
        f.base = head;
        write(f.repo.path(), "src/pages/signup.ts", "another fix");
        let head = commit(f.repo.path());
        assert!(f.run(&head).unwrap_err().contains("BUG FIX WITHOUT A CASE"));
    }
}

#[test]
fn registry_upkeep_observe_persists_would_refusal_and_off_skips() {
    // A delivery that breaks YAML retains the base policy's severity, including
    // a tracked header that overrides a different local configuration.
    for (header, local, expected) in [
        (
            None,
            RegistryEnforcement::Observe,
            RegistryEnforcement::Observe,
        ),
        (None, RegistryEnforcement::Off, RegistryEnforcement::Off),
        (
            Some("observe"),
            RegistryEnforcement::Strict,
            RegistryEnforcement::Observe,
        ),
        (
            Some("enforce"),
            RegistryEnforcement::Off,
            RegistryEnforcement::Enforce,
        ),
    ] {
        let yaml = header.map_or_else(
            || YAML.to_owned(),
            |stage| format!("enforce: {stage}\n{YAML}"),
        );
        let mut broken = Fixture::new(&yaml);
        broken.qa.registry.enforce = local;
        write(
            broken.repo.path(),
            crate::qa_registry::DEFAULT_PATH,
            "invalid: [",
        );
        let head = commit(broken.repo.path());
        let result = broken.run(&head);
        match expected {
            RegistryEnforcement::Off => {
                assert!(result.unwrap().is_empty());
                assert!(broken.store.get(&broken.task.id).unwrap().notes.is_empty());
            }
            RegistryEnforcement::Observe => {
                result.unwrap();
                let notes = broken.store.get(&broken.task.id).unwrap().notes;
                assert!(
                    notes.contains("UPKEEP OBSERVE G1")
                        && notes.contains("JOURNEY REGISTRY BROKEN"),
                    "{notes}"
                );
            }
            _ => assert!(result.unwrap_err().contains("JOURNEY REGISTRY BROKEN")),
        }
    }

    let mut config = crate::config::Config::default();
    let key = "qa.registry.enforce";
    assert_eq!(config.get(key).as_deref(), Some("enforce"));
    assert!(crate::config::meta::registry().get(key).is_some());
    for stage in ["off", "observe", "enforce", "strict"] {
        config.set(key, stage).unwrap();
        assert_eq!(config.get(key).as_deref(), Some(stage));
        assert!(config.list().contains(&(key.into(), stage.into())));
    }
    assert!(config.set(key, "automatic").is_err());
    let mut f = Fixture::new(YAML);
    f.qa.registry.enforce = RegistryEnforcement::Observe;
    write(f.repo.path(), "src/pages/new.ts", "entry");
    let head = commit(f.repo.path());
    f.run(&head).unwrap();
    let notes = f.store.get(&f.task.id).unwrap().notes;
    assert!(
        notes.contains("UPKEEP OBSERVE G2") && notes.contains("UNOWNED USER-FACING FILE"),
        "{notes}"
    );
    f.run(&head).unwrap();
    assert_eq!(f.store.get(&f.task.id).unwrap().notes, notes);
    f.qa.registry.enforce = RegistryEnforcement::Off;
    f.run(&head).unwrap();
    assert_eq!(f.store.get(&f.task.id).unwrap().notes, notes);

    // A tracked policy is authoritative, including over a local off shortcut.
    write(
        f.repo.path(),
        crate::qa_registry::DEFAULT_PATH,
        &format!("enforce: enforce\n{YAML}"),
    );
    let enforced = commit(f.repo.path());
    assert!(
        f.run(&enforced)
            .unwrap_err()
            .contains("UNOWNED USER-FACING FILE")
    );
    // Resolve at the delivered head; a different checkout policy cannot relax it.
    write(
        f.repo.path(),
        crate::qa_registry::DEFAULT_PATH,
        &format!("enforce: off\n{YAML}"),
    );
    assert!(
        f.run(&enforced)
            .unwrap_err()
            .contains("UNOWNED USER-FACING FILE")
    );
    f.base = enforced;
    write(f.repo.path(), "src/pages/relaxed.ts", "new entry");
    let disabled = commit(f.repo.path());
    // A delivery cannot relax its own gate: base enforce beats head off.
    assert!(
        f.run(&disabled)
            .unwrap_err()
            .contains("UNOWNED USER-FACING FILE")
    );
    // The relaxed header takes effect only for a later delivery.
    f.base = disabled;
    write(f.repo.path(), "src/pages/later.ts", "later entry");
    let later = commit(f.repo.path());
    f.qa.registry.enforce = RegistryEnforcement::Strict;
    f.run(&later).unwrap();

    let mut absent = Fixture::new(YAML);
    git(
        absent.repo.path(),
        &["rm", crate::qa_registry::DEFAULT_PATH],
    );
    absent.base = commit(absent.repo.path());
    absent.qa.registry.enforce = RegistryEnforcement::Off;
    let same_tip = absent.base.clone();
    assert!(absent.run(&same_tip).unwrap().is_empty());
}

#[test]
fn registry_upkeep_malformed_untouched_registry_passes_with_warning() {
    // Repairing an unreadable base cannot disable the gate in the same delivery,
    // even when the local fallback is off; strict local policy must also survive.
    for stage in [RegistryEnforcement::Off, RegistryEnforcement::Strict] {
        let mut repair = Fixture::new("invalid: [");
        if stage == RegistryEnforcement::Strict {
            write(repair.repo.path(), "src/pages/legacy.ts", "old");
            repair.base = commit(repair.repo.path());
        }
        repair.qa.registry.enforce = stage;
        write(repair.repo.path(), "src/pages/legacy.ts", "changed");
        write(
            repair.repo.path(),
            crate::qa_registry::DEFAULT_PATH,
            &format!("enforce: off\n{YAML}"),
        );
        let head = commit(repair.repo.path());
        let error = repair.run(&head).unwrap_err();
        assert!(
            error.contains("UNOWNED USER-FACING FILE") && error.contains("legacy.ts"),
            "{stage:?}: {error}"
        );
    }

    let mut f = Fixture::new("invalid: [");
    write(f.repo.path(), "src/pages/new.ts", "entry");
    let head = commit(f.repo.path());
    assert!(f.run(&head).unwrap().join("\n").contains("not blocking"));
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("not blocking")
    );
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, "still: [");
    let head = commit(f.repo.path());
    assert!(
        f.run(&head)
            .unwrap_err()
            .contains("JOURNEY REGISTRY BROKEN")
    );
    write(f.repo.path(), crate::qa_registry::DEFAULT_PATH, YAML);
    assert!(
        f.run(&head).is_err(),
        "uncommitted repair cannot change the delivery snapshot"
    );
    f.decision("registry unchanged: existing parse debt is being migrated");
    f.run(&head).unwrap();
}

#[test]
fn registry_upkeep_catalog_drift_refuses_with_missing_journey() {
    let mut f = Fixture::new(&format!("selection: legacy_catalog\n{YAML}"));
    write(
        f.repo.path(),
        "scripts/journeys-for-diff.py",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/journeys-for-diff.py"
        )),
    );
    // The catalog changes but the registry does not: selected metadata is absent.
    write(
        f.repo.path(),
        "docs/qa/journeys.md",
        "## hub-web\n\n### HUB-J99 · New entry\n\n- **Touches:** `src/pages/signup.ts`\n",
    );
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("JOURNEY REGISTRY BROKEN")
            && error.contains("HUB-J99")
            && error.contains(crate::qa_registry::DEFAULT_PATH),
        "{error}"
    );
    f.qa.registry.enforce = RegistryEnforcement::Observe;
    f.run(&head).unwrap();
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("UPKEEP OBSERVE G1")
    );
}
