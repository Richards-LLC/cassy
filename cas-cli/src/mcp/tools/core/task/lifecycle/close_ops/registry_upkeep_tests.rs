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
    fn run(&mut self, head: &str) -> Result<(), String> {
        check(
            &self.store,
            &mut self.task,
            self.repo.path(),
            &self.qa,
            &self.base,
            head,
            None,
            None,
        )
    }
    fn decision(&mut self, body: &str) {
        append_close_decision_note(&self.store, &mut self.task, &format!("✅ DECISION {body}"));
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
    let mut f = Fixture::new(YAML);
    write(f.repo.path(), "src/pages/legacy.ts", "old");
    f.base = commit(f.repo.path());
    write(f.repo.path(), "src/pages/legacy.ts", "changed");
    let head = commit(f.repo.path());
    f.run(&head).unwrap();
    f.qa.registry.enforce = RegistryEnforcement::Strict;
    assert!(f.run(&head).unwrap_err().contains("src/pages/legacy.ts"));
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
    f.run(&head).unwrap();
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
        &f.store,
        &mut f.task,
        f.repo.path(),
        &f.qa,
        &f.base,
        &head,
        Some(&["README.md".into()]),
        None,
    )
    .unwrap();
}

#[test]
fn registry_upkeep_bug_requires_changed_case_for_task() {
    let mut f = Fixture::new(YAML);
    f.task.task_type = TaskType::Bug;
    f.store.update(&f.task).unwrap();
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
        f.store.update(&f.task).unwrap();
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
}

#[test]
fn registry_upkeep_malformed_untouched_registry_passes_with_warning() {
    let mut f = Fixture::new("invalid: [");
    write(f.repo.path(), "src/pages/new.ts", "entry");
    let head = commit(f.repo.path());
    f.run(&head).unwrap();
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
    f.decision("registry unchanged: existing parse debt is being migrated");
    f.run(&head).unwrap();
}
