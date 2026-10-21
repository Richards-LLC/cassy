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
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q", "-b", "main"]);
        write(repo.path(), "src/account/start.ts", "entry");
        write(repo.path(), "schema/model.sql", "model");
        write(
            repo.path(),
            ".claude/CODEMAP.md",
            "- `src/account/` — Accounts\n- `schema/model.sql` — Model\n",
        );
        write(repo.path(), "docs/PRODUCT_OVERVIEW.md", "# Product\n");
        write(
            repo.path(),
            ".cas/config.toml",
            "[project_overview]\ndomain_paths = ['schema/model.sql']\nwatch_dirs = ['src/pages']\n",
        );
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
        f.run(&head).is_ok(),
        "a new file in an existing directory is advisory"
    );
    write(f.repo.path(), "src/payments/start.ts", "new directory");
    let head = commit(f.repo.path());
    let error = f.run(&head).unwrap_err();
    assert!(
        error.contains("CODEMAP DRIFT") && error.contains("src/payments"),
        "{error}"
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
    for rename in [false, true] {
        let mut f = Fixture::new();
        if rename {
            git(
                f.repo.path(),
                &["mv", "schema/model.sql", "schema/entity.sql"],
            );
        } else {
            git(f.repo.path(), &["rm", "schema/model.sql"]);
        }
        let head = commit(f.repo.path());
        let error = f.run(&head).unwrap_err();
        assert!(
            error.contains("CODEMAP DRIFT") && error.contains("schema/model.sql"),
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
    for change in ["domain", "live", "dead", "watched"] {
        let mut f = Fixture::new();
        if change == "dead" {
            write(
                f.repo.path(),
                crate::qa_registry::DEFAULT_PATH,
                "version: 1\njourneys:\n  - {id: signup, status: live}\n",
            );
            f.base = commit(f.repo.path());
        }
        match change {
            "domain" => write(f.repo.path(), "schema/model.sql", "new domain model"),
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
        assert!(f.run(&head).is_ok());
        assert!(f.task.notes.contains("UPKEEP ESCAPE G6"));
    }
}
