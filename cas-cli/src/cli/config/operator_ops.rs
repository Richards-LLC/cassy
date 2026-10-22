//! Operator-only write roots and per-task write grants (cas-3147, GH #1169,
//! GH #1182).
//!
//! `cas config set factory.write_roots` writes `[factory] write_roots` in the
//! project's `.cas/config.toml`; it refuses only a factory worker cgroup, so
//! the operator's `! cas …` inside an agent pane works. The grant commands
//! write `.cas/operator/write-policy.toml`; they refuse any agent context (see
//! [`operator_context_refusal`]) and ask the operator to confirm at the
//! terminal. The PreToolUse hook refuses agent tool calls that run these
//! commands or write either file.

use std::io::{BufRead, Write};
use std::path::Path;

use crate::cli::Cli;
use crate::config::operator_policy::{
    InvocationContext, load_operator_policy, operator_context_refusal, operator_policy_path,
    parse_modes, parse_project_write_roots, resolve_operator_path, resolve_project_write_root,
    save_operator_policy,
};

/// `cas config grant-write`: a one-off write grant bound to one open task.
///
/// Operator-only, guardrail-grade (not security-grade): agents share your Unix
/// user, so Cassy's tool-call hook is what stops them; this command's own
/// agent checks are defence in depth.
#[derive(clap::Parser)]
#[command(
    after_help = "Guardrail-grade, not security-grade: the operator and the agents share a Unix user. Cassy's PreToolUse hook refuses agents that run this command or write .cas/operator/; the checks here (no agent environment, no agent ancestor, no factory cgroup, a terminal, a typed confirmation) are defence in depth."
)]
pub struct ConfigGrantWriteArgs {
    /// Task the grant is bound to; it ends when the task closes
    #[arg(long)]
    pub task: String,
    /// Directory (or file) outside the worktree the task may write
    #[arg(long)]
    pub path: String,
    /// Modes, e.g. create+edit (default) or create+edit+delete
    #[arg(long, default_value = "create+edit")]
    pub mode: String,
    /// Why the operator grants it; recorded on the task
    #[arg(long)]
    pub reason: String,
}

/// `cas config revoke-write`: remove a task's grants (all, or one path).
#[derive(clap::Parser)]
pub struct ConfigRevokeWriteArgs {
    #[arg(long)]
    pub task: String,
    #[arg(long)]
    pub path: Option<String>,
}

/// cas-0d4f0: refuse to save `after` over `before` when that changes a
/// security-relevant key and `context` (read only when one changes) is not
/// the operator at a terminal. Every config save path calls this.
pub(crate) fn guard_operator_config(
    before: &crate::config::Config,
    after: &crate::config::Config,
    context: impl FnOnce() -> InvocationContext,
) -> anyhow::Result<()> {
    let changed = crate::config::operator_policy::changed_operator_config_keys(before, after);
    if changed.is_empty() {
        return Ok(());
    }
    if let Some(refusal) =
        crate::config::operator_policy::operator_config_refusal(&changed, &context())
    {
        anyhow::bail!(refusal);
    }
    Ok(())
}

fn require_operator() -> anyhow::Result<()> {
    if let Some(refusal) = operator_context_refusal(&InvocationContext::from_process()) {
        anyhow::bail!(refusal);
    }
    Ok(())
}

fn confirm(prompt: &str, expected: &str) -> anyhow::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "{prompt}\nType `{expected}` to confirm: ")?;
    out.flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    if line.trim() != expected {
        anyhow::bail!("not confirmed; nothing changed");
    }
    Ok(())
}

fn record_event(cas_root: &Path, event: &str, fields: &[(&str, &str)]) {
    let _ = crate::hooks::handlers::session_hygiene::append_factory_session_event(cas_root, event, fields);
}

/// `cas config set factory.write_roots <value>` (GH #1182).
///
/// Writes `[factory] write_roots` in the project's `.cas/config.toml`. The
/// operator usually runs this as `! cas …` inside an agent pane, which
/// carries agent environment and has no terminal, so the only refusal here is
/// a factory worker cgroup; agents running it as a tool call are refused by
/// the PreToolUse hook. Roots left in the cas-3147 operator policy file are
/// moved into config.toml (an empty value clears both).
pub(crate) fn execute_set_write_roots(value: &str, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let change = set_project_write_roots(value, cas_root, &cgroup)?;
    let config_path = cas_root.join("config.toml").display().to_string();
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "key": "factory.write_roots",
                "old_value": change.previous,
                "new_value": change.stored,
                "migrated_from_operator_policy": change.migrated,
                "dropped_from_operator_policy": change.dropped,
                "path": config_path,
            })
        );
        return Ok(());
    }
    if change.stored.is_empty() {
        println!("Cleared factory.write_roots in .cas/config.toml");
    } else {
        println!("Set factory.write_roots in .cas/config.toml.");
        println!("Agents may create, edit and delete files beneath:");
        for root in &change.stored {
            println!("  {root}");
        }
    }
    if !change.migrated.is_empty() {
        println!("Moved from .cas/operator/write-policy.toml:");
        for path in &change.migrated {
            println!("  {path}");
        }
    }
    if !change.dropped.is_empty() {
        eprintln!("Dropped legacy roots that are no longer allowed:");
        for entry in &change.dropped {
            eprintln!("  {entry}");
        }
    }
    Ok(())
}

/// What `cas config set factory.write_roots` changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRootsChange {
    pub previous: Vec<String>,
    pub stored: Vec<String>,
    /// Roots moved from the cas-3147 operator policy file into config.toml.
    pub migrated: Vec<String>,
    /// Legacy roots that no longer pass validation, with the reason.
    pub dropped: Vec<String>,
}

/// Validate `value`, migrate legacy roots and write `[factory] write_roots`
/// to `cas_root/config.toml`. `cgroup` is the caller's `/proc/self/cgroup`;
/// a factory worker or server cgroup is refused.
pub(crate) fn set_project_write_roots(
    value: &str,
    cas_root: &Path,
    cgroup: &str,
) -> anyhow::Result<WriteRootsChange> {
    if let Some(refusal) = crate::config::operator_policy::worker_cgroup_refusal(cgroup) {
        anyhow::bail!(refusal);
    }
    let mut roots = parse_project_write_roots(value, cas_root)?;
    let clearing = roots.is_empty();

    // Migrate legacy cas-3147 roots out of the operator policy file; an empty value
    // clears them too. Grants stay in the file.
    let mut legacy = load_operator_policy(cas_root)?;
    let mut migrated = Vec::new();
    let mut dropped = Vec::new();
    if !clearing {
        for root in &legacy.roots {
            match resolve_project_write_root(&root.path.display().to_string(), cas_root) {
                Ok(path) if !roots.contains(&path) => {
                    migrated.push(path.display().to_string());
                    roots.push(path);
                }
                Ok(_) => {}
                Err(error) => dropped.push(format!("{} ({error})", root.path.display())),
            }
        }
    }

    let mut config = crate::config::Config::load(cas_root)?;
    let stored: Vec<String> = roots.iter().map(|root| root.display().to_string()).collect();
    let previous = crate::cli::config::read_ops::update_config_in_place(
        &mut config,
        "factory.write_roots",
        cas_root,
        |before, after| {
            let previous = before.factory().write_roots;
            after.set_factory_write_roots(stored.clone());
            Ok(previous)
        },
        // The worker-cgroup refusal above is this key's operator-only guard:
        // operator commands from an agent pane are intentionally supported.
        |_, _| Ok(()),
    )?;

    if !legacy.roots.is_empty() {
        legacy.roots.clear();
        if legacy.grants.is_empty() {
            match std::fs::remove_file(operator_policy_path(cas_root)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        } else {
            save_operator_policy(cas_root, &legacy)?;
        }
    }

    let joined = stored.join(",");
    record_event(cas_root, "operator_write_roots_set", &[("roots", joined.as_str())]);
    Ok(WriteRootsChange { previous, stored, migrated, dropped })
}

/// `cas config get factory.write_roots`: the config.toml value, plus a
/// note on stderr for roots still honoured from the cas-3147 operator policy file.
pub(crate) fn execute_get_write_roots(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = crate::config::Config::load(cas_root)?;
    let roots = config.factory().write_roots;
    let legacy: Vec<String> = load_operator_policy(cas_root)
        .map(|policy| policy.roots.iter().map(|root| root.path.display().to_string()).collect())
        .unwrap_or_default();
    if cli.json {
        println!(
            "{}",
            serde_json::json!({"key": "factory.write_roots", "value": roots, "legacy_operator_policy_roots": legacy})
        );
        return Ok(());
    }
    println!("{}", roots.join(","));
    if !legacy.is_empty() {
        eprintln!("note: also honoured from .cas/operator/write-policy.toml:");
        for root in &legacy {
            eprintln!("  {root}");
        }
        eprintln!("`cas config set factory.write_roots` moves them into config.toml.");
    }
    Ok(())
}

/// `cas config grant-write`.
pub(crate) fn execute_grant_write(args: &ConfigGrantWriteArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    require_operator()?;
    if args.reason.trim().is_empty() {
        anyhow::bail!("--reason is required: it is recorded on the task");
    }
    let task_store = crate::store::open_task_store(cas_root)?;
    let task = task_store.get(&args.task)?;
    if task.status == cas_types::TaskStatus::Closed {
        anyhow::bail!("task {} is closed; a grant ends when its task closes", task.id);
    }
    let path = resolve_operator_path(&args.path, cas_root)?;
    // Validate before asking the operator to confirm.
    parse_modes(&args.mode)?;
    confirm(
        &format!(
            "Grant agents working on {} ({}) write access to {} ({}) until the task closes.\nReason: {}",
            task.id,
            task.title,
            path.display(),
            args.mode,
            args.reason.trim()
        ),
        &path.display().to_string(),
    )?;
    let grant = crate::config::operator_policy::record_operator_grant(
        cas_root,
        task_store.as_ref(),
        &task.id,
        &path.display().to_string(),
        &args.mode,
        &args.reason,
        &crate::config::operator_policy::GrantSource::OperatorCli,
    )?;
    let path = grant.path;
    let path_text = path.display().to_string();
    if cli.json {
        println!("{}", serde_json::json!({"task": task.id, "path": path_text, "mode": args.mode}));
    } else {
        println!("Granted {path_text} ({}) to {} until it closes", args.mode, task.id);
    }
    Ok(())
}

/// `cas config revoke-write`.
pub(crate) fn execute_revoke_write(args: &ConfigRevokeWriteArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    require_operator()?;
    let task_store = crate::store::open_task_store(cas_root)?;
    let removed = crate::config::operator_policy::revoke_operator_grants(
        cas_root,
        task_store.as_ref(),
        &args.task,
        args.path.as_deref(),
        &crate::config::operator_policy::GrantSource::OperatorCli,
    )?;
    if cli.json {
        println!("{}", serde_json::json!({"task": args.task, "removed": removed}));
    } else {
        println!("Revoked {removed} grant(s) for {}", args.task);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_default_roundtrip_value(
        key: &str,
        meta: &crate::config::ConfigMeta,
        before: &crate::config::Config,
    ) -> String {
        let mut candidates = Vec::new();
        match meta.value_type {
            crate::config::ConfigType::Bool => {
                candidates.extend(["true".to_owned(), "false".to_owned()]);
            }
            crate::config::ConfigType::Int => {
                if let Ok(default) = meta.default.parse::<i64>() {
                    for delta in [1, -1, 2, -2] {
                        if let Some(value) = default.checked_add(delta) {
                            candidates.push(value.to_string());
                        }
                    }
                }
                candidates.extend(["0", "1", "2", "5", "10", "60", "100"].into_iter().map(str::to_owned));
            }
            crate::config::ConfigType::Float => {
                if let Ok(default) = meta.default.parse::<f64>() {
                    candidates.extend([default + 0.25, default - 0.25].into_iter().map(|value| value.to_string()));
                }
                candidates.extend(["0.25", "0.5", "0.75", "1.25"].into_iter().map(str::to_owned));
            }
            crate::config::ConfigType::String => {}
            crate::config::ConfigType::StringList => {
                candidates.extend(["roundtrip-one", "roundtrip-one,roundtrip-two"].into_iter().map(str::to_owned));
            }
        }
        if let crate::config::Constraint::OneOf(values) = &meta.constraint {
            candidates.extend(values.iter().cloned());
        }
        candidates.extend(
            [
                "roundtrip-value",
                "roundtrip-model",
                "roundtrip/path",
                "example/repo",
                "/tmp/cas-config-roundtrip",
                "https://example.com",
                "auto",
                "enabled",
                "disabled",
                "claude",
                "codex",
                "low",
                "medium",
                "high",
                "required",
                "loopback",
                "cloud",
                "warn",
                "retrieval,helpful",
                "[]",
                r#"[{"workspace_id":"T123","user_id":"U123","minimum_interval_secs":60,"mode":"warn"}]"#,
            ]
            .into_iter()
            .map(str::to_owned),
        );
        candidates.dedup();

        let before_value = before.get(key);
        for candidate in &candidates {
            if candidate == meta.default || meta.validate(candidate).is_err() {
                continue;
            }
            let mut probe = before.clone();
            if probe.set(key, candidate).is_ok() && probe.get(key) != before_value {
                return candidate.clone();
            }
        }

        candidates
            .into_iter()
            .find(|candidate| candidate != meta.default && meta.validate(candidate).is_ok())
            .unwrap_or_else(|| panic!("no valid non-default round-trip value for {key}"))
    }
    use crate::cli::config::read_ops::set_config_value;
    use crate::config::Config;
    use crate::config::operator_policy::{
        OperatorWriteGrant, OperatorWriteMode, OperatorWritePolicy, OperatorWriteRoot,
    };
    use crate::test_support::TestEnvGuard;

    fn agent() -> InvocationContext {
        InvocationContext {
            env_names: ["HOME", "CLAUDECODE"].into_iter().map(String::from).collect(),
            ancestors: vec!["-bash".into()],
            ..InvocationContext::default()
        }
    }

    fn operator() -> InvocationContext {
        InvocationContext {
            env_names: ["HOME", "PATH"].into_iter().map(String::from).collect(),
            ancestors: vec!["-bash".into()],
            stdin_is_terminal: true,
            stdout_is_terminal: true,
            cgroup: String::new(),
        }
    }

    #[test]
    fn issue_repository_set_rejects_invalid_slugs_without_saving_cas_d3e9() {
        let _env = TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut config = Config::load(&root).unwrap();
        for key in ["issues.repo", "issues.components.cassy", "issues.components.violet", "issues.components.cloud"] {
            let default = Config::default().get(key).unwrap();
            set_config_value(&mut config, key, " owner/repo.name-1_2 ", &root, agent).unwrap();
            assert_eq!(Config::load(&root).unwrap().get(key).as_deref(), Some("owner/repo.name-1_2"));
            let stored = std::fs::read(root.join("config.toml")).unwrap();
            for invalid in ["bad/repo/extra", "owner", "/repo", "owner/", "own er/repo", "owner/repo\nother", "https://github.com/owner/repo"] {
                let error = set_config_value(&mut config, key, invalid, &root, agent)
                    .expect_err("malformed repository must be refused before saving").to_string();
                assert!(error.contains("owner/name"), "{key}: {error}");
                assert_eq!(config.get(key).as_deref(), Some("owner/repo.name-1_2"), "invalid write mutated {key}");
                assert_eq!(std::fs::read(root.join("config.toml")).unwrap(), stored, "invalid write saved {key}");
            }
            set_config_value(&mut config, key, "  ", &root, agent).unwrap();
            assert_eq!(Config::load(&root).unwrap().get(key).as_deref(), Some(default.as_str()), "blank clears {key}");
        }
    }

    /// cas-0d4f0: `cas config set` refuses a security-relevant key from an
    /// agent and leaves config.toml unchanged; ordinary keys stay settable,
    /// and the operator at a terminal can still set the key.
    #[test]
    fn cas_0d4f0_config_set_refuses_operator_keys_from_agents() {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        let mut config = Config::load(&root).unwrap();
        let error = set_config_value(&mut config, "slack.transport", "any", &root, agent)
            .unwrap_err()
            .to_string();
        assert!(error.contains("slack.transport") && error.contains("operator"), "{error}");
        let mut config = Config::load(&root).unwrap();
        let error = set_config_value(&mut config, "verification.enabled", "false", &root, agent)
            .unwrap_err()
            .to_string();
        assert!(error.contains("verification.enabled"), "{error}");
        assert_ne!(
            Config::load(&root).unwrap().get("verification.enabled").as_deref(),
            Some("false")
        );
        assert_ne!(Config::load(&root).unwrap().get("slack.transport").as_deref(), Some("any"));

        let mut config = Config::load(&root).unwrap();
        set_config_value(&mut config, "issues.repo", "example/project", &root, agent).unwrap();
        assert_eq!(
            Config::load(&root).unwrap().get("issues.repo").as_deref(),
            Some("example/project")
        );

        let mut config = Config::load(&root).unwrap();
        set_config_value(&mut config, "slack.transport", "any", &root, operator).unwrap();
        assert_eq!(Config::load(&root).unwrap().get("slack.transport").as_deref(), Some("any"));
    }

    #[test]
    fn ct2_p3_commander_mirror_config_set_is_operator_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut config = Config::default();
        config.save(root).unwrap();
        let original = std::fs::read_to_string(root.join("config.toml")).unwrap();
        let error = set_config_value(&mut config, "factory.commander_mirror_enabled", "false", root, agent)
            .expect_err("agents cannot disable automatic replies").to_string();
        assert!(error.contains("factory.commander_mirror_enabled") && error.contains("operator"));
        assert_eq!(std::fs::read_to_string(root.join("config.toml")).unwrap(), original);
        let mut config = Config::load(root).unwrap();
        set_config_value(&mut config, "factory.commander_mirror_enabled", "false", root, operator).unwrap();
        let original = std::fs::read_to_string(root.join("config.toml")).unwrap();
        let error = set_config_value(&mut config, "factory.commander_mirror_enabled", "true", root, agent)
            .expect_err("agents cannot undo the operator opt-out").to_string();
        assert!(error.contains("operator"));
        assert_eq!(std::fs::read_to_string(root.join("config.toml")).unwrap(), original);
        let mut config = Config::load(root).unwrap();
        set_config_value(&mut config, "factory.commander_mirror_enabled", "true", root, operator).unwrap();
        assert!(Config::load(root).unwrap().factory().commander_mirror_enabled);
    }

    /// cas-0d4f0: reset, import and the line editor save through the same
    /// guard, so a whole-config replacement cannot relax a guard either.
    #[test]
    fn cas_0d4f0_whole_config_saves_refuse_operator_key_changes() {
        let before = Config::default();
        let mut after = before.clone();
        after.set("verification.force_bypass_allowed", "true").unwrap();
        assert!(guard_operator_config(&before, &after, agent).is_err());
        assert!(guard_operator_config(&before, &after, operator).is_ok());
        let mut ordinary = before.clone();
        ordinary.set("sync.min_helpful", "5").unwrap();
        assert!(
            guard_operator_config(&before, &ordinary, || panic!("context read for ordinary key"))
                .is_ok()
        );
    }

    /// The cgroup of an operator's terminal tab or the supervisor's pane.
    const PANE_CGROUP: &str =
        "0::/user.slice/user@1000.service/app.slice/app-org.kde.konsole-1.scope/tab(2).scope";

    /// An operator `! cas …` inside an agent pane: agent environment, no
    /// terminal (the test harness has none either).
    fn operator_in_agent_pane() -> TestEnvGuard {
        let mut env = TestEnvGuard::temp_home();
        for (key, value) in [
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CAS_AGENT_ROLE", "supervisor"),
            ("CAS_SESSION_ID", "sess-1"),
            ("CAS_FACTORY_MODE", "1"),
        ] {
            env.set(key, value);
        }
        env
    }

    /// GH #1182 acceptance 3: the operator's `! cas config set
    /// factory.write_roots` from a pane with agent env and no TTY succeeds,
    /// writes config.toml with canonical paths, `get`/`list` show it, and an
    /// empty value clears it.
    #[test]
    fn gh_1182_operator_pane_sets_and_clears_write_roots_in_config_toml() {
        let env = operator_in_agent_pane();
        let home = env.home().to_path_buf();
        let project = home.join("projects/a");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let downloads = home.join("Downloads");
        let models = home.join("mnt-models");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&models).unwrap();
        std::os::unix::fs::symlink(&models, home.join("models-link")).unwrap();

        let config_path = cas_root.join("config.toml");
        let original = "# operator-maintained config\n\
[sync]\n\
min_helpful = 3 # preserve sync value\n\
\n\
[factory]\n\
# compatibility spelling stays in place\n\
worker_build_jobs = \"4\"\n";
        std::fs::write(&config_path, original).unwrap();

        let change = set_project_write_roots(
            &format!("~/Downloads, {}", home.join("models-link").display()),
            &cas_root,
            PANE_CGROUP,
        )
        .expect("the operator's pane command succeeds");
        let expected = vec![downloads.display().to_string(), models.display().to_string()];
        assert_eq!(change.stored, expected, "`~` expands and symlinks resolve");
        assert!(change.previous.is_empty());

        let expected_toml = original.replace(
            "worker_build_jobs = \"4\"\n",
            &format!(
                "worker_build_jobs = \"4\"\nwrite_roots = [\"{}\", \"{}\"]\n",
                expected[0],
                expected[1],
            ),
        );
        let text = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(text, expected_toml);
        assert!(text.contains("write_roots"), "{text}");
        let config = crate::config::Config::load(&cas_root).unwrap();
        assert_eq!(config.factory().write_roots, expected);
        assert_eq!(config.get("factory.write_roots"), Some(expected.join(",")));
        let listed: std::collections::HashMap<String, String> = config.list().into_iter().collect();
        assert_eq!(listed.get("factory.write_roots"), Some(&expected.join(",")));
        let cli = <Cli as clap::Parser>::parse_from(["cas", "config", "get", "factory.write_roots"]);
        execute_get_write_roots(&cli, &cas_root).unwrap();

        let change = set_project_write_roots("", &cas_root, PANE_CGROUP).unwrap();
        assert_eq!(change.previous, expected);
        assert!(change.stored.is_empty());
        assert!(crate::config::Config::load(&cas_root).unwrap().factory().write_roots.is_empty());
        let text = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(text, original);
        assert!(!text.contains("write_roots"), "{text}");
    }

    #[test]
    fn config_registry_keys_round_trip_through_set_and_get() {
        let env = operator_in_agent_pane();
        let home = env.home().to_path_buf();
        let project = home.join("projects/roundtrip");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let downloads = home.join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let documents = home.join("Documents");
        std::fs::create_dir_all(&documents).unwrap();

        let original = format!(
            "# existing aliases\n\
[factory]\n\
worker_build_jobs = \"4\"\n\
write_roots = [\"{}\"]\n\
\n\
[staging]\n\
large_artifact_dir = \"before\"\n",
            downloads.display()
        );
        std::fs::write(cas_root.join("config.toml"), original).unwrap();

        let registry = crate::config::registry();
        let mut keys = registry.all_keys();
        keys.extend(["factory.cargo_build_jobs", "staging.large_artifact_dir"]);
        keys.sort_unstable();
        keys.dedup();

        for key in keys {
            let mut config = Config::load(&cas_root).unwrap();
            let meta = registry
                .get(key)
                .or_else(|| match key {
                    "factory.cargo_build_jobs" => registry.get("factory.worker_build_jobs"),
                    "staging.large_artifact_dir" => registry.get("staging.staging_dir"),
                    _ => None,
                })
                .expect("registered key metadata or alias");
            let value = match key {
                "factory.worker_build_jobs" => "8".to_owned(),
                "factory.cargo_build_jobs" => "9".to_owned(),
                "staging.staging_dir" => "roundtrip-staging-dir".to_owned(),
                "staging.large_artifact_dir" => "roundtrip-large-artifact-dir".to_owned(),
                "factory.write_roots" => "~/Documents".to_owned(),
                // The registry intentionally permits only `low` for the
                // shared worker, so assert its serialized path instead of
                // inventing an invalid non-default value.
                "factory.ai_enrichment.effort" => "low".to_owned(),
                "llm.supervisor.harness" => "codex".to_owned(),
                "llm.supervisor.model" => "roundtrip-supervisor-model".to_owned(),
                "llm.supervisor.reasoning_effort" => "medium".to_owned(),
                "llm.worker.harness" => "claude".to_owned(),
                "llm.worker.model" => "roundtrip-worker-model".to_owned(),
                "llm.worker.reasoning_effort" => "high".to_owned(),
                _ => non_default_roundtrip_value(key, meta, &config),
            };

            if key == "factory.write_roots" {
                let before = config.clone();
                let change = set_project_write_roots(&value, &cas_root, PANE_CGROUP).unwrap();
                let mut expected = before.clone();
                expected.set_factory_write_roots(change.stored.clone());
                let actual = Config::load(&cas_root).unwrap();
                assert_ne!(actual.get(key), before.get(key), "{key} must use a non-default value");
                assert_eq!(actual.get(key), expected.get(key), "{key}");
                continue;
            }

            let before_value = config.get(key);
            let mut expected = config.clone();
            expected
                .set(key, &value)
                .unwrap_or_else(|error| panic!("unexpected set rejection for {key}: {error}"));
            if key != "factory.ai_enrichment.effort" {
                assert_ne!(expected.get(key), before_value, "{key} must use a non-default value");
            }
            set_config_value(&mut config, key, &value, &cas_root, operator).unwrap();
            let actual = Config::load(&cas_root).unwrap();
            assert_eq!(actual.get(key), expected.get(key), "{key} = {value}");
            if key == "factory.ai_enrichment.effort" {
                let saved = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
                assert!(saved.contains("[factory.ai_enrichment]"), "{saved}");
                assert!(saved.contains("effort = \"low\""), "{saved}");
            }
            if key == "factory.worker_build_jobs" {
                let saved = std::fs::read_to_string(cas_root.join("config.toml")).unwrap();
                assert!(saved.contains("worker_build_jobs = \"8\""), "{saved}");
                assert!(!saved.contains("cargo_build_jobs ="), "{saved}");
            }
        }
    }

    /// GH #1182: setting roots refuses inside a factory worker (or server)
    /// cgroup, and refuses unsafe roots without touching config.toml.
    #[test]
    fn gh_1182_set_refuses_worker_cgroups_and_unsafe_roots() {
        let env = operator_in_agent_pane();
        let home = env.home().to_path_buf();
        let project = home.join("projects/a");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        std::fs::create_dir_all(cas_root.join("worktrees")).unwrap();
        let before = std::fs::read_to_string(cas_root.join("config.toml")).ok();

        for cgroup in [
            "0::/user.slice/app.slice/tab(2).scope/cas-worker-cassy-agile-raven-93-golden-finch-99",
            "0::/user.slice/app.slice/cas-server-cassy-daemon",
        ] {
            let error = set_project_write_roots("~/Downloads", &cas_root, cgroup).unwrap_err().to_string();
            assert!(error.contains("cgroup"), "{error}");
        }
        std::os::unix::fs::symlink(&home, home.join("home-link")).unwrap();
        for unsafe_root in [
            "/".to_string(),
            "~".to_string(),
            home.display().to_string(),
            home.parent().unwrap().display().to_string(),
            "relative/dir".to_string(),
            "~/Downloads/../Downloads".to_string(),
            home.join("home-link").display().to_string(),
            project.display().to_string(),
            cas_root.display().to_string(),
            cas_root.join("worktrees").display().to_string(),
            home.join("missing").display().to_string(),
            "~/Downloads:create+edit".to_string(),
        ] {
            assert!(
                set_project_write_roots(&unsafe_root, &cas_root, PANE_CGROUP).is_err(),
                "{unsafe_root} must be refused"
            );
        }
        assert_eq!(std::fs::read_to_string(cas_root.join("config.toml")).ok(), before);
    }

    /// GH #1182 acceptance 6: setting roots moves legacy cas-3147 roots out of the
    /// operator policy file into config.toml and keeps per-task grants there;
    /// a file left with only roots is removed.
    #[test]
    fn gh_1182_set_migrates_legacy_roots_and_keeps_grants() {
        let env = operator_in_agent_pane();
        let home = env.home().to_path_buf();
        let project = home.join("projects/a");
        let cas_root = crate::store::init_cas_dir(&project).unwrap();
        let legacy_root = home.join("soundwave-config/docs/requests");
        let granted = home.join("granted");
        let downloads = home.join("Downloads");
        for dir in [&legacy_root, &granted, &downloads] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let grant = OperatorWriteGrant {
            task: "cas-1".into(),
            path: granted.clone(),
            modes: [OperatorWriteMode::Create].into_iter().collect(),
            reason: "INGEST".into(),
            granted_at: "2026-10-13T00:00:00Z".into(),
            granted_by: Some("operator-cli".into()),
        };
        save_operator_policy(
            &cas_root,
            &OperatorWritePolicy {
                roots: vec![OperatorWriteRoot {
                    path: legacy_root.clone(),
                    modes: [OperatorWriteMode::Create].into_iter().collect(),
                }],
                grants: vec![grant.clone()],
            },
        )
        .unwrap();

        let change = set_project_write_roots("~/Downloads", &cas_root, PANE_CGROUP).unwrap();
        assert_eq!(change.migrated, vec![legacy_root.display().to_string()]);
        assert_eq!(
            change.stored,
            vec![downloads.display().to_string(), legacy_root.display().to_string()]
        );
        let policy = load_operator_policy(&cas_root).unwrap();
        assert!(policy.roots.is_empty(), "roots left the legacy file");
        assert_eq!(policy.grants, vec![grant], "grants are unchanged");

        // A legacy file holding only roots is removed once they move.
        save_operator_policy(
            &cas_root,
            &OperatorWritePolicy {
                roots: vec![OperatorWriteRoot {
                    path: legacy_root.clone(),
                    modes: [OperatorWriteMode::Edit].into_iter().collect(),
                }],
                grants: Vec::new(),
            },
        )
        .unwrap();
        set_project_write_roots("~/Downloads", &cas_root, PANE_CGROUP).unwrap();
        assert!(!operator_policy_path(&cas_root).exists());
    }

    /// GH #1182: `cas config import` never changes write roots.
    #[test]
    fn gh_1182_config_import_keeps_current_write_roots() {
        let env = operator_in_agent_pane();
        let home = env.home().to_path_buf();
        let cas_root = crate::store::init_cas_dir(&home.join("projects/a")).unwrap();
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        set_project_write_roots("~/Downloads", &cas_root, PANE_CGROUP).unwrap();
        let forged = home.join("forged.yaml");
        std::fs::write(&forged, "factory:\n  write_roots:\n    - /\n  note_max_chars: 1700\n").unwrap();
        let cli = <Cli as clap::Parser>::parse_from([
            "cas",
            "config",
            "import",
            forged.to_str().unwrap(),
            "--force",
        ]);
        let Some(crate::cli::Commands::Config(crate::cli::config::ConfigCommands::Import(args))) = &cli.command else {
            panic!("import args");
        };
        crate::cli::config::io_ops::execute_import(args, &cli, &cas_root).unwrap();
        let config = crate::config::Config::load(&cas_root).unwrap();
        assert_eq!(config.factory().note_max_chars, 1700, "other keys import");
        assert_eq!(config.factory().write_roots, vec![home.join("Downloads").display().to_string()]);
    }

}
