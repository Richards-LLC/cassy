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
        Self::resolve(cwd, cas_root, false)
    }

    /// Pre-enforcement reports measure integration, including when HEAD is ahead.
    pub fn integration_at(cwd: &Path, cas_root: &Path) -> Self {
        Self::resolve(cwd, cas_root, true)
    }

    fn resolve(cwd: &Path, cas_root: &Path, integration_only: bool) -> Self {
        let root = crate::store::detect::find_git_toplevel(cwd).unwrap_or_else(|_| {
            let hint = cas_root.parent().unwrap_or(cwd);
            let current = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
            let project = hint.canonicalize().unwrap_or_else(|_| hint.to_path_buf());
            if current.starts_with(&project) {
                hint.to_path_buf()
            } else if cwd.is_dir() {
                cwd.to_path_buf()
            } else {
                hint.to_path_buf()
            }
        });
        let store = if cas_root.parent().is_some_and(|hint| root.as_path() == hint || same_repo(&root, hint)) {
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
        let mut reference = if integration_only || exists(&remote) {
            remote
        } else if exists(target) {
            target.into()
        } else {
            "HEAD".into()
        };
        // A worker ahead of integration must see its own new changes. A stale
        // or divergent checkout must not hide drift on the integration ref.
        if !integration_only
            && reference != "HEAD"
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
        Self::resolve_store(cas_root, false)
    }

    pub fn integration_for_store(cas_root: &Path) -> Self {
        Self::resolve_store(cas_root, true)
    }

    fn resolve_store(cas_root: &Path, integration_only: bool) -> Self {
        let hint = cas_root.parent().unwrap_or(cas_root);
        let cwd = std::env::current_dir()
            .ok()
            .filter(|cwd| same_repo(cwd, hint));
        Self::resolve(cwd.as_deref().unwrap_or(hint), cas_root, integration_only)
    }

    pub fn read(&self, path: &str) -> Option<String> {
        if self.reference == "HEAD" {
            std::fs::read_to_string(self.root.join(path)).ok()
        } else {
            git(&self.root, &["show", &format!("{}:{path}", self.reference)])
        }
    }

    pub fn last_commit(&self, path: &str) -> Option<String> {
        self.log_value(path, "--format=%H")
    }

    pub fn last_updated(&self, path: &str) -> Option<String> {
        self.log_value(path, "--format=%ci")
    }

    fn log_value(&self, path: &str, format: &str) -> Option<String> {
        git(
            &self.root,
            &["log", "-1", format, &self.reference, "--", path],
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

#[derive(serde::Serialize)]
pub(crate) struct ArtifactDrift {
    pub name: &'static str,
    pub path: String,
    pub drift_count: Option<usize>,
    pub unit: &'static str,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debt_count: Option<usize>,
}

impl ArtifactDrift {
    fn unknown(name: &'static str, path: &str, unit: &'static str) -> Self {
        Self {
            name,
            path: path.into(),
            drift_count: None,
            unit,
            state: "unknown",
            detail: None,
            debt_count: None,
        }
    }

    fn observe(&mut self, count: Option<usize>) {
        self.drift_count = count;
        self.state = match count {
            None => "missing",
            Some(0) => "current",
            Some(_) => "drift",
        };
    }
}

/// Shared integration inventory for status and post-save enforcement warnings.
/// Curation debt is deliberately not registry drift or a predicted close refusal.
#[derive(serde::Serialize)]
pub(crate) struct UpkeepReport {
    pub measured_ref: String,
    pub codemap: ArtifactDrift,
    pub product_overview: ArtifactDrift,
    pub registry: ArtifactDrift,
}

impl UpkeepReport {
    pub fn at(measurement: &Measurement, qa: &crate::config::QaConfig) -> Self {
        use super::project_overview::ProjectOverviewStaleness;
        let sha = git(
            &measurement.root,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", measurement.reference),
            ],
        );
        let mut report = Self {
            measured_ref: format!(
                "{}@{}",
                measurement.reference,
                sha.as_deref().map(str::trim).unwrap_or("unavailable")
            ),
            codemap: ArtifactDrift::unknown(
                "CODEMAP",
                ".claude/CODEMAP.md",
                "structural change(s)",
            ),
            product_overview: ArtifactDrift::unknown(
                "PRODUCT_OVERVIEW",
                super::project_overview::DOC_PATH,
                "relevant change(s)",
            ),
            registry: ArtifactDrift::unknown(
                "journey registry",
                &qa.registry.path,
                "registry issue(s)",
            ),
        };
        let Some(sha) = sha else {
            return report;
        };
        // Keep every read on the named commit even if another process fetches
        // or advances the integration ref while this report is being built.
        let snapshot = Measurement {
            root: measurement.root.clone(),
            store: measurement.store.clone(),
            reference: sha.trim().into(),
        };
        let measurement = &snapshot;
        if let Some(evaluation) = super::codemap::evaluate_codemap_at(measurement) {
            report.codemap.observe(evaluation.structural_change_count());
        }
        let watch_warning = measurement
            .read(super::project_overview::DOC_PATH)
            .and_then(|overview| {
                // Store config is commonly untracked. When the measured commit
                // has no config, watch targets use the current live store config;
                // that fallback is not pinned to the measured SHA.
                let config = measurement
                    .read(".cas/config.toml")
                    .or_else(|| std::fs::read_to_string(measurement.store.join("config.toml")).ok())
                    .and_then(|text| super::project_overview::config_section(&text));
                super::project_overview::resolve_watch_targets(config, Some(&overview), || {
                    measurement.tracked()
                })
                .warning
            });
        if let Some(warning) = watch_warning {
            report.product_overview.detail = Some(warning);
        } else {
            match super::project_overview::check_freshness_at(measurement) {
                Ok(staleness) => report.product_overview.observe(match staleness {
                    None => Some(0),
                    Some(ProjectOverviewStaleness::Missing) => None,
                    Some(
                        ProjectOverviewStaleness::Stale { total_changes, .. }
                        | ProjectOverviewStaleness::SignificantlyStale { total_changes, .. },
                    ) => Some(total_changes),
                }),
                Err(error) => report.product_overview.detail = Some(error.to_string()),
            }
        }
        let Some(tracked) = git(
            &measurement.root,
            &["ls-tree", "-rz", "--name-only", &measurement.reference],
        ) else {
            return report;
        };
        let tracked: Vec<String> = tracked
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        let registry = measurement
            .read(&qa.registry.path)
            .map(|text| crate::qa_registry::parse(&text));
        let status = crate::qa_seed::status_with_registry(qa, registry, &tracked);
        if status.present {
            report
                .registry
                .observe(Some(status.problems.len() + status.unowned_surfaces.len()));
            if !status.valid {
                report.registry.state = "invalid";
            }
            report.registry.debt_count = Some(status.debt_count);
            report.registry.detail = Some(format!(
                "{} validation problem(s), {} unowned surface file(s)",
                status.problems.len(),
                status.unowned_surfaces.len()
            ));
        } else {
            report.registry.observe(None);
        }
        report
    }

    pub fn for_key(&self, key: &str) -> Option<&ArtifactDrift> {
        match key {
            "upkeep.codemap" => Some(&self.codemap),
            "upkeep.product_overview" => Some(&self.product_overview),
            "qa.registry.enforce" => Some(&self.registry),
            _ => None,
        }
    }

    pub fn artifacts(&self) -> [&ArtifactDrift; 3] {
        [&self.codemap, &self.product_overview, &self.registry]
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
            "SELECT deliverables FROM tasks WHERE status = 'in_progress'
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_finds_documents_generated_only_in_level_worktree() {
        if let Ok(document) = std::env::var("CAS_930C_STATUS") {
            use clap::Parser;
            let cli = crate::cli::Cli::parse_from(["cas", &document, "status"]);
            crate::cli::run(cli).unwrap();
            return;
        }
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let fixture = tempfile::tempdir().unwrap();
        let repo = fixture.path().canonicalize().unwrap().join("main");
        let worker = repo.join(".cas/worktrees/worker");
        let store = repo.join(".cas");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("config.toml"), "[factory]\nepic_base_branch = \"main\"\n").unwrap();
        crate::store::SqliteTaskStore::open(&store).unwrap();
        let run_git = |root: &Path, args: &[&str]| {
            let output = Command::new("git").args(args).current_dir(root).output().unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(&repo, &["config", "user.name", "Fixture"]);
        run_git(&repo, &["config", "user.email", "fixture@example.test"]);
        std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
        run_git(&repo, &["add", ".gitignore"]);
        run_git(&repo, &["commit", "-qm", "initial"]);
        run_git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run_git(&repo, &["worktree", "add", "-qb", "factory/worker", worker.to_str().unwrap()]);
        std::fs::create_dir_all(worker.join(".claude")).unwrap();
        std::fs::create_dir_all(worker.join("docs")).unwrap();
        std::fs::write(worker.join(".claude/CODEMAP.md"), "# Worker map\n").unwrap();
        std::fs::write(worker.join("docs/PRODUCT_OVERVIEW.md"), "# Worker product\n").unwrap();
        env.set("CAS_ROOT", &store);

        // Exercise the real status dispatch from a nested caller directory.
        let name = concat!(module_path!(), "::status_finds_documents_generated_only_in_level_worktree");
        let name = name.strip_prefix("cas::").unwrap_or(name);
        let mut failures = Vec::new();
        for (cwd, expected, root) in [
            (worker.join("docs"), "Status: up to date", &worker),
            (repo.clone(), "Status: not found", &repo),
        ] {
            for (command, path) in [("codemap", ".claude/CODEMAP.md"), ("project-overview", "docs/PRODUCT_OVERVIEW.md")] {
                let output = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", name, "--nocapture"])
                    .current_dir(&cwd).env("CAS_930C_STATUS", command).output().unwrap();
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(output.status.success(), "{stdout} {}", String::from_utf8_lossy(&output.stderr));
                crate::test_child::assert_passed(&stdout, name);
                if !stdout.contains(expected) || !stdout.contains(&root.join(path).display().to_string()) {
                    failures.push(format!("{command} from {} expected {expected}: {stdout}", cwd.display()));
                }
            }
        }
        // Integration reports must still describe committed origin, not local docs.
        let integration = Measurement::integration_at(&worker, &store);
        for path in [".claude/CODEMAP.md", "docs/PRODUCT_OVERVIEW.md"] {
            assert!(integration.read(path).is_none());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
