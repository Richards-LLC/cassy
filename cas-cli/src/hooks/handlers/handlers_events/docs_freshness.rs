//! Read-only tree/ref selection shared by documentation status and hook checks.
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) struct Measurement {
    pub root: PathBuf,
    pub store: PathBuf,
    pub reference: String,
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn same_repo(a: &Path, b: &Path) -> bool {
    let common = |root| {
        git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
            .map(|text| {
                let path = PathBuf::from(text.trim());
                path.canonicalize().unwrap_or(path)
            })
    };
    common(a).zip(common(b)).is_some_and(|(a, b)| a == b)
}

impl Measurement {
    /// Hooks supply the harness cwd; CLI status supplies the process cwd.
    pub fn at(cwd: &Path, cas_root: &Path) -> Self {
        let root = crate::store::detect::find_git_toplevel(cwd)
            .unwrap_or_else(|_| cas_root.parent().unwrap_or(cwd).to_path_buf());
        let store = if cas_root.parent().is_some_and(|hint| same_repo(&root, hint)) {
            cas_root.to_path_buf()
        } else {
            root.join(".cas")
        };
        let target = task_target(&root, &store)
            .or_else(|| crate::config::Config::configured_epic_base_branch(store.parent()?))
            .unwrap_or_else(|| {
                crate::worktree::git::GitOperations::new(root.clone()).detect_default_branch()
            });
        let target = target.strip_prefix("origin/").unwrap_or(&target);
        let remote = format!("origin/{target}");
        let exists = |reference: &str| {
            git(
                &root,
                &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
            )
            .is_some()
        };
        let mut reference = if exists(&remote) {
            remote
        } else if exists(target) {
            target.into()
        } else {
            "HEAD".into()
        };
        // A worker ahead of integration must see its own new changes. A stale
        // or divergent checkout must not hide drift on the integration ref.
        if reference != "HEAD"
            && git(&root, &["merge-base", "--is-ancestor", &reference, "HEAD"]).is_some()
            && git(&root, &["rev-parse", &reference]) != git(&root, &["rev-parse", "HEAD"])
        {
            reference = "HEAD".into();
        }
        Self {
            root,
            store,
            reference,
        }
    }

    /// Preserve explicit fixture/project roots while letting shared-store
    /// consumers follow a cwd in another linked checkout of that repository.
    pub fn for_store(cas_root: &Path) -> Self {
        let hint = cas_root.parent().unwrap_or(cas_root);
        let cwd = std::env::current_dir()
            .ok()
            .filter(|cwd| same_repo(cwd, hint));
        Self::at(cwd.as_deref().unwrap_or(hint), cas_root)
    }

    pub fn read(&self, path: &str) -> Option<String> {
        if self.reference == "HEAD" {
            std::fs::read_to_string(self.root.join(path)).ok()
        } else {
            git(&self.root, &["show", &format!("{}:{path}", self.reference)])
        }
    }

    pub fn last_commit(&self, path: &str) -> Option<String> {
        git(
            &self.root,
            &["log", "-1", "--format=%H", &self.reference, "--", path],
        )
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
    }

    pub fn tracked(&self) -> Vec<String> {
        git(
            &self.root,
            &["ls-tree", "-rz", "--name-only", &self.reference],
        )
        .unwrap_or_default()
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
    }

    pub fn annotate(&self, injection: String) -> String {
        injection.replacen(
            ">\n",
            &format!(
                ">\nMeasured ref: {} ({}).\n",
                self.reference,
                self.root.display()
            ),
            1,
        )
    }
}

/// A status read never initializes or migrates the shared store. Only one
/// unambiguous current assignment may supply the task's integration target.
fn task_target(root: &Path, store: &Path) -> Option<String> {
    use rusqlite::{Connection, OpenFlags};
    let identity = std::env::var("CAS_AGENT_NAME")
        .or_else(|_| std::env::var("CAS_AGENT_ID"))
        .ok()?;
    let conn =
        Connection::open_with_flags(store.join("cas.db"), OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut identities = vec![identity.clone()];
    if let Ok(mut agents) = conn.prepare("SELECT id FROM agents WHERE name = ?1 LIMIT 2") {
        if let Ok(rows) = agents.query_map([&identity], |row| row.get::<_, String>(0)) {
            identities.extend(rows.filter_map(Result::ok));
        }
    }
    let mut statement = conn
        .prepare(
            "SELECT deliverables FROM tasks WHERE status IN ('in_progress', 'open', 'blocked')
         AND assignee IN (SELECT value FROM json_each(?1)) LIMIT 2",
        )
        .ok()?;
    let rows: Vec<String> = statement
        .query_map([serde_json::to_string(&identities).ok()?], |row| row.get(0))
        .ok()?
        .collect::<Result<_, _>>()
        .ok()?;
    if rows.len() != 1 {
        return None;
    }
    let deliverables: cas_types::TaskDeliverables = serde_json::from_str(&rows[0]).ok()?;
    let target = deliverables.work_target?;
    let matches = |repo| {
        crate::mcp::tools::core::task::repo_context::repo_answers_to(repo, &target.repo_selector)
    };
    (matches(root) || store.parent().is_some_and(matches)).then_some(target.target_branch)
}
