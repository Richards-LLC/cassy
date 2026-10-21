use super::*;
use cas_store::TaskStore;
use std::process::Command;

struct Fixture {
    repo: tempfile::TempDir,
    db: tempfile::TempDir,
    store: cas_store::SqliteTaskStore,
    task: Task,
    base: String,
}
fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "docs fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "docs fixture")
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
    fn new() -> Self {
        Self::with(&[
            ("src/account/start.ts", "entry"),
            ("schema/model.sql", "model"),
            (
                ".claude/CODEMAP.md",
                "- `src/account/` — Accounts\n- `schema/model.sql` — Model\n",
            ),
            ("docs/PRODUCT_OVERVIEW.md", "# Product\n"),
            (
                ".cas/config.toml",
                "[project_overview]\ndomain_paths = ['schema/model.sql']\nwatch_dirs = ['src/pages']\n",
            ),
        ])
    }
    fn with(files: &[(&str, &str)]) -> Self {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        for (path, body) in files {
            write(repo.path(), path, body);
        }
        let base = commit(repo.path());
        let db = tempfile::tempdir().unwrap();
        let store = cas_store::SqliteTaskStore::open(db.path()).unwrap();
        store.init().unwrap();
        let task = Task::new("cas-1234".into(), "Product change".into());
        store.add(&task).unwrap();
        Self {
            repo,
            db,
            store,
            task,
            base,
        }
    }
    fn run(&mut self, head: &str) -> Result<Vec<String>, String> {
        at_park(
            &self.store,
            &mut self.task,
            self.db.path(),
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
}

#[test]
fn docs_upkeep_enumerated_directory_requires_map_or_reason() {
    let mut f = Fixture::new();
    write(f.repo.path(), "src/account/extra.ts", "existing directory");
    let head = commit(f.repo.path());
    assert!(
        f.run(&head)
            .unwrap()
            .iter()
            .any(|n| n.contains("UPKEEP ADVISORY G5") && n.contains("src/account/extra.ts")),
        "a new file in an existing directory is advisory"
    );
    write(f.repo.path(), "src/payments/start.ts", "new directory");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("CODEMAP DRIFT") && error.contains("src/payments"),
        "{error}"
    );
    let mut config = crate::config::Config::default();
    config.set("upkeep.codemap", "observe").unwrap();
    config.save(f.db.path()).unwrap();
    assert!(
        f.run(&head)
            .unwrap()
            .iter()
            .any(|n| n.contains("UPKEEP OBSERVE G5"))
    );
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("UPKEEP OBSERVE G5")
    );
    config.set("upkeep.codemap", "off").unwrap();
    config.save(f.db.path()).unwrap();
    assert!(f.run(&head).is_ok());
    config.set("upkeep.codemap", "enforce").unwrap();
    config.save(f.db.path()).unwrap();
    append_close_decision_note(
        &f.store,
        &mut f.task,
        "📝 PROGRESS codemap unchanged: quoted only",
    );
    assert!(f.run(&head).is_err());
    assert!(
        at_park(
            &f.store,
            &mut f.task,
            f.db.path(),
            f.repo.path(),
            &f.base,
            None,
            Some(&head),
            Some("reviewed by supervisor")
        )
        .is_ok()
    );
    assert!(
        f.run(&head).is_err(),
        "a previous override does not authorize a later call"
    );
    f.decision("codemap unchanged:");
    assert!(f.run(&head).is_err());
    f.decision("codemap unchanged: payments belong to the documented account subsystem");
    assert!(f.run(&head).is_ok());
    assert!(
        f.store
            .get(&f.task.id)
            .unwrap()
            .notes
            .contains("UPKEEP ESCAPE G5")
    );
    // Working-copy edits do not repair a committed delivery.
    let mut f = Fixture::new();
    write(f.repo.path(), "src/payments/start.ts", "new directory");
    let head = commit(f.repo.path());
    write(
        f.repo.path(),
        ".claude/CODEMAP.md",
        "- src/payments — Payments\n",
    );
    assert!(f.run(&head).is_err());
    let head = commit(f.repo.path());
    assert!(f.run(&head).is_ok());
}

#[test]
fn docs_upkeep_removed_named_path_requires_map_edit() {
    for (old, new, rename) in [
        ("schema/model.sql", "schema/entity.sql", false),
        ("schema/model.sql", "schema/entity.sql", true),
        ("src/account", "src/profile", false),
        ("src/account", "src/profile", true),
    ] {
        let mut f = Fixture::new();
        if rename {
            git(f.repo.path(), &["mv", old, new]);
        } else {
            git(f.repo.path(), &["rm", "-r", old]);
        }
        let head = commit(f.repo.path());
        let error = f.run(&head).unwrap_err();
        assert!(
            error.contains("CODEMAP DRIFT") && error.contains(old),
            "{error}"
        );
        write(
            f.repo.path(),
            ".claude/CODEMAP.md",
            "- src/account — Accounts\n",
        );
        write(
            f.repo.path(),
            "docs/PRODUCT_OVERVIEW.md",
            "# Updated model\n",
        );
        let head = commit(f.repo.path());
        assert!(f.run(&head).is_ok());
    }
}

#[test]
fn docs_upkeep_product_change_requires_overview_or_reason() {
    for change in ["domain", "live", "dead", "revived", "watched", "malformed"] {
        let mut f = Fixture::new();
        if change == "dead" || change == "revived" {
            write(
                f.repo.path(),
                crate::qa_registry::DEFAULT_PATH,
                &format!(
                    "version: 1\njourneys:\n  - {{id: signup, status: {}}}\n",
                    if change == "dead" { "live" } else { "dead" }
                ),
            );
            f.base = commit(f.repo.path());
        }
        if change == "watched" {
            write(f.repo.path(), "src/pages/existing.ts", "existing page");
            f.base = commit(f.repo.path());
        }
        if change == "malformed" {
            write(
                f.repo.path(),
                crate::qa_registry::DEFAULT_PATH,
                "invalid: [",
            );
            f.base = commit(f.repo.path());
        }
        match change {
            "domain" | "malformed" => write(f.repo.path(), "schema/model.sql", "new domain model"),
            "watched" => write(f.repo.path(), "src/pages/start.ts", "new flow"),
            _ => write(
                f.repo.path(),
                crate::qa_registry::DEFAULT_PATH,
                &format!(
                    "version: 1\njourneys:\n  - {{id: signup, status: {}}}\n",
                    if change == "dead" { "dead" } else { "live" }
                ),
            ),
        }
        let head = commit(f.repo.path());
        // Prove WatchTargets and documents come from the delivered snapshot.
        write(
            f.repo.path(),
            ".cas/config.toml",
            "[project_overview]\ndomain_paths=[]\nwatch_dirs=[]\n",
        );
        let error = f.run(&head).unwrap_err();
        assert!(
            error.contains("PRODUCT_OVERVIEW DRIFT"),
            "{change}: {error}"
        );
        f.decision("overview unchanged:");
        assert!(f.run(&head).is_err());
        f.decision("overview unchanged: existing flow description already covers this model");
        let result = f.run(&head);
        assert!(result.is_ok(), "{change}: {result:?}");
        assert!(f.task.notes.contains("UPKEEP ESCAPE G6"));
    }
    let mut f = Fixture::new();
    write(f.repo.path(), "schema/model.sql", "changed model");
    write(
        f.repo.path(),
        "docs/PRODUCT_OVERVIEW.md",
        "# Updated domain model\n",
    );
    let head = commit(f.repo.path());
    assert!(f.run(&head).is_ok());
}

#[test]
fn docs_upkeep_overview_watch_targets_come_from_the_tracked_overview() {
    // cas-src shape: .cas/ is ignored, so no config travels with the delivery.
    let overview = "# Product\n<!-- cas:project-overview\ndomain_paths = ['crates/cas-types/src/task.rs']\nwatch_dirs = ['cas-cli/src/cli']\n-->\n";
    let files = [
        (".gitignore", ".cas/\n"),
        ("cas-cli/src/cli/mod.rs", "cli"),
        ("crates/cas-types/src/task.rs", "task"),
        ("docs/PRODUCT_OVERVIEW.md", overview),
    ];
    let mut f = Fixture::with(&files);
    write(f.repo.path(), "cas-cli/src/cli/new_command.rs", "new surface");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("PRODUCT_OVERVIEW DRIFT") && error.contains("cas-cli/src/cli/new_command.rs"),
        "{error}"
    );
    // A local project config override still wins over the tracked block.
    let mut config = crate::config::Config::default();
    config.project_overview = Some(crate::config::ProjectOverviewConfig {
        domain_paths: Some(vec![]),
        watch_dirs: Some(vec![]),
    });
    config.save(f.db.path()).unwrap();
    assert!(f.run(&head).is_ok());

    // Nothing declared and nothing derivable: park passes but says so.
    let mut f = Fixture::with(&[
        ("cas-cli/src/cli/mod.rs", "cli"),
        ("docs/PRODUCT_OVERVIEW.md", "# Product\n"),
    ]);
    write(f.repo.path(), "cas-cli/src/cli/new_command.rs", "new surface");
    let head = commit(f.repo.path());
    let notices = f.run(&head).unwrap();
    assert!(
        notices
            .iter()
            .any(|n| n.starts_with("UPKEEP WARNING G6: no product paths are watched")),
        "{notices:?}"
    );
}
