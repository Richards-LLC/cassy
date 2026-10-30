//! Role-scoped native skill listings for factory workers.
//!
//! Each harness renders its own listing from the whole installed catalog, about
//! 12,000 to 18,000 characters per launch. Workers keep only
//! [`cas_pty::WORKER_ROLE_SKILLS`] in that listing, through the harness's own
//! control; `docs/analysis/2026-10-28-native-skill-catalog-projection.md` holds
//! the measurements. Everything stays reachable by name: a typed `/name` or
//! `$name` still expands, and the `skill` MCP tool lists and serves the catalog.
//! Supervisors keep the full listing.
//!
//! Failures here cost tokens, not correctness, so they log and launch anyway.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use cas_pty::WORKER_ROLE_SKILLS;
use serde_json::{Map, Value};

use crate::pty::PtyConfig;

/// Claude Code's own bundled skills, which `--settings` can hide by name.
const CLAUDE_BUNDLED_SKILLS: &[&str] = &[
    "claude-api",
    "code-review",
    "dataviz",
    "fewer-permission-prompts",
    "init",
    "keybindings-help",
    "loop",
    "plugin-authoring",
    "run",
    "security-review",
    "simplify",
    "update-config",
    "workflow-authoring",
];

const EXPLICIT_ONLY: &str = "disable-model-invocation: true";
const GROK_PROJECT_SKILLS: &str = ".grok/skills";

fn is_role_skill(name: &str) -> bool {
    WORKER_ROLE_SKILLS.contains(&name)
}

/// Directory names only: a private skill's contents are never read here.
fn skill_names(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// Whether `cwd` is a Cassy-created worker checkout: a direct child of
/// `<project>/.cas/worktrees`. This gates Grok's trust bypass, so the store must
/// be `.cas`; a user's own `worktrees/<name>` directory does not qualify.
pub(super) fn is_cassy_worktree(cwd: &Path) -> bool {
    cas_types::factory_scratch::managed_worktree_store(cwd)
        .and_then(Path::file_name)
        .is_some_and(|store| store == ".cas")
}

/// The Claude account a worker runs under, resolved as the child resolves it.
pub(super) fn claude_account(config_dir: Option<&str>) -> Option<PathBuf> {
    config_dir
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
}

/// Hide every non-role skill from Claude's listing. `user-invocable-only` keeps
/// `/name` working. A second `--settings` flag would replace the first, so the
/// worker's hooks and permissions merge with the overrides into one value.
pub(super) fn apply_claude(config: &mut PtyConfig, account: &Path, cwd: &Path) {
    let mut names: BTreeSet<String> = CLAUDE_BUNDLED_SKILLS.iter().map(|s| s.to_string()).collect();
    names.extend(skill_names(&account.join("skills")));
    names.extend(skill_names(&cwd.join(".claude/skills")));
    let overrides: Map<String, Value> = names
        .into_iter()
        .filter(|name| !is_role_skill(name))
        .map(|name| (name, Value::String("user-invocable-only".into())))
        .collect();

    let Some(flag) = config.args.iter().position(|arg| arg == "--settings") else {
        let settings = serde_json::json!({ "skillOverrides": overrides });
        config.args.extend(["--settings".to_string(), settings.to_string()]);
        return;
    };
    let path = &config.args[flag + 1];
    let parsed = std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|error| error.to_string()));
    match parsed {
        Ok(Value::Object(mut settings)) => {
            settings.insert("skillOverrides".into(), Value::Object(overrides));
            config.args[flag + 1] = Value::Object(settings).to_string();
        }
        Ok(_) => tracing::warn!(path, "worker settings are not an object; full skill listing kept"),
        Err(error) => tracing::warn!(path, %error, "worker settings unreadable; full skill listing kept"),
    }
}

/// Grok lists a project skill ahead of a same-named personal one, so an
/// explicit-only copy in the worker's checkout hides the personal row and keeps
/// `/name` working. A Cassy-managed copy already in the checkout gets the same
/// treatment. A user's own skills stay listed, whether personal or in the project,
/// and so do skills the repository tracks: changing those would dirty the checkout.
pub(super) fn prepare_grok_project_skills(cwd: &Path, grok_home: &Path) -> std::io::Result<usize> {
    if !is_cassy_worktree(cwd) {
        return Ok(0);
    }
    let target = cwd.join(GROK_PROJECT_SKILLS);
    let tracked = git_lines(cwd, &["ls-files", "--", GROK_PROJECT_SKILLS]);
    let is_tracked = |name: &str| {
        let prefix = format!("{GROK_PROJECT_SKILLS}/{name}/");
        tracked.iter().any(|file| file.starts_with(&prefix))
    };
    let mut projected = BTreeSet::new();
    for skill in skill_dirs(&grok_home.join("skills")) {
        let Some(name) = projectable_name(&skill).filter(|name| !is_tracked(name)) else {
            continue;
        };
        let Ok(body) = std::fs::read_to_string(skill.join("SKILL.md")) else {
            continue;
        };
        let destination = target.join(&name);
        let customized = std::fs::read_to_string(destination.join("SKILL.md"))
            .is_ok_and(|existing| !is_cassy_managed(&existing));
        if !is_cassy_managed(&body) || customized {
            continue;
        }
        copy_tree(&skill, &destination)?;
        std::fs::write(destination.join("SKILL.md"), explicit_only(&body))?;
        projected.insert(name);
    }
    for skill in skill_dirs(&target) {
        let Some(name) = projectable_name(&skill)
            .filter(|name| !is_tracked(name) && !projected.contains(name))
        else {
            continue;
        };
        let file = skill.join("SKILL.md");
        let Ok(body) = std::fs::read_to_string(&file) else {
            continue;
        };
        if is_cassy_managed(&body) {
            std::fs::write(file, explicit_only(&body))?;
            projected.insert(name);
        }
    }
    if !projected.is_empty() {
        ignore_projection(cwd, &target)?;
    }
    Ok(projected.len())
}

fn skill_dirs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

/// A skill directory's name, unless it is a role skill and so stays listed.
fn projectable_name(skill: &Path) -> Option<String> {
    let name = skill.file_name()?.to_str()?;
    (!is_role_skill(name)).then(|| name.to_string())
}

fn is_cassy_managed(content: &str) -> bool {
    frontmatter_end(content).is_some_and(|end| {
        let frontmatter = &content[..end];
        frontmatter.contains("managed_by: cas") || frontmatter.contains("managed_by: \"cas\"")
    })
}

/// Byte offset of the newline before the closing `---`.
fn frontmatter_end(content: &str) -> Option<usize> {
    let rest = content.strip_prefix("---\n")?;
    rest.find("\n---").map(|end| end + 4)
}

fn explicit_only(content: &str) -> String {
    let Some(end) = frontmatter_end(content) else {
        return content.to_string();
    };
    let lines: Vec<&str> = content[..end]
        .lines()
        .filter(|line| !line.starts_with("disable-model-invocation:"))
        .collect();
    format!("{}\n{EXPLICIT_ONLY}{}", lines.join("\n"), &content[end..])
}

/// Regular files and directories only; a symlink could lead out of the skill.
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)?.flatten() {
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn git_lines(cwd: &Path, args: &[&str]) -> Vec<String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Keep the projection out of this checkout's `git status`. The directory ignores
/// itself because Git has no per-worktree exclude: it reads `info/exclude` only
/// from the common dir, which would also hide the main checkout's untracked files.
fn ignore_projection(cwd: &Path, target: &Path) -> std::io::Result<()> {
    let probe = format!("{GROK_PROJECT_SKILLS}/probe/SKILL.md");
    let marker = target.join(".gitignore");
    if git_lines(cwd, &["check-ignore", &probe]).is_empty() && !marker.exists() {
        std::fs::write(marker, "*\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{SupervisorLaunchConfig, WorkerLaunchConfig};
    use crate::harness::SupervisorCli;

    const MANAGED: &str = "---\nname: x\ndescription: d\nmetadata:\n  managed_by: cas\n---\nBODY\n";

    fn git(root: &Path, args: &[&str]) {
        assert!(Command::new("git").arg("-C").arg(root).args(args).output().unwrap().status.success());
    }

    fn worker_config(cli: SupervisorCli, cwd: &Path, account: &Path) -> PtyConfig {
        cli.backend().build_worker_config(WorkerLaunchConfig {
            name: "probe",
            cwd: cwd.to_path_buf(),
            cas_root: None,
            supervisor_name: "sup",
            supervisor_cli: SupervisorCli::Claude,
            model: None,
            effort: None,
            config_dir: Some(account.to_str().unwrap()),
            config_dir_source: Some("explicit"),
            secure_storage_dir: None,
            teams: None,
            active_workers: None,
        })
    }

    /// Every harness shrinks a worker's listing through its own control, and
    /// none of them removes the named route the worker contract advertises.
    #[test]
    fn every_harness_scopes_the_worker_listing() {
        let dir = tempfile::tempdir().unwrap();
        let account = dir.path().join("account");
        std::fs::create_dir_all(account.join("skills/cas-worker")).unwrap();
        std::fs::create_dir_all(account.join("skills/private-note")).unwrap();

        let claude = worker_config(SupervisorCli::Claude, dir.path(), &account);
        let at = claude.args.iter().position(|arg| arg == "--settings").unwrap();
        let settings: Value = serde_json::from_str(&claude.args[at + 1]).unwrap();
        assert_eq!(settings["skillOverrides"]["private-note"], "user-invocable-only");
        assert_eq!(settings["skillOverrides"]["init"], "user-invocable-only");
        assert!(settings["skillOverrides"].get("cas-worker").is_none());

        let codex = worker_config(SupervisorCli::Codex, dir.path(), &account);
        assert!(codex.args.windows(2).any(|pair| pair == ["-c", "skills.include_instructions=false"]));

        let worktree = dir.path().join(".cas/worktrees/probe");
        let grok = worker_config(SupervisorCli::Grok, &worktree, &account);
        assert!(grok.env.iter().any(|(key, value)| key == "GROK_FOLDER_TRUST" && value == "false"));

        let opencode = worker_config(SupervisorCli::OpenCode, dir.path(), &account);
        let inline = opencode.env.iter().find(|(key, _)| key == "OPENCODE_CONFIG_CONTENT").unwrap();
        let inline: Value = serde_json::from_str(&inline.1).unwrap();
        let skill = &inline["agent"]["cassy-worker"]["permission"]["skill"];
        assert_eq!(skill["*"], "deny");
        for name in WORKER_ROLE_SKILLS {
            assert_eq!(skill[*name], "allow", "{name}");
        }
    }

    /// The trust bypass and its project copies stay inside Cassy's own worker
    /// checkouts: never the main checkout, a lookalike path, or a supervisor.
    #[test]
    fn grok_trust_bypass_is_refused_outside_a_cassy_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("skills/cas-release-report")).unwrap();
        std::fs::write(home.join("skills/cas-release-report/SKILL.md"), MANAGED).unwrap();
        let bypass = |config: &PtyConfig| config.env.iter().any(|(key, _)| key == "GROK_FOLDER_TRUST");

        for refused in [dir.path().join("main"), dir.path().join("own/worktrees/feature")] {
            std::fs::create_dir_all(&refused).unwrap();
            assert!(!bypass(&worker_config(SupervisorCli::Grok, &refused, &home)), "{}", refused.display());
            assert_eq!(prepare_grok_project_skills(&refused, &home).unwrap(), 0);
            assert!(!refused.join(GROK_PROJECT_SKILLS).exists(), "{}", refused.display());
        }

        let worktree = dir.path().join("main/.cas/worktrees/probe");
        assert!(bypass(&worker_config(SupervisorCli::Grok, &worktree, &home)));
        let supervisor = SupervisorCli::Grok.backend().build_supervisor_config(SupervisorLaunchConfig {
            name: "sup",
            cwd: worktree,
            cas_root: None,
            worker_cli: SupervisorCli::Grok,
            worker_names: &[],
            model: None,
            effort: None,
            teams: None,
        });
        assert!(!bypass(&supervisor));
    }

    #[test]
    fn claude_merge_keeps_worker_hooks_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("worker-settings.json");
        std::fs::write(&file, r#"{"hooks":{"Stop":[]},"permissions":{"allow":["Read"]}}"#).unwrap();
        let mut config = worker_config(SupervisorCli::Grok, dir.path(), dir.path());
        config.args = vec!["--settings".into(), file.to_str().unwrap().into()];
        apply_claude(&mut config, dir.path(), dir.path());
        let settings: Value = serde_json::from_str(&config.args[1]).unwrap();
        assert_eq!(settings["permissions"]["allow"][0], "Read");
        assert!(settings["hooks"].get("Stop").is_some());
        assert_eq!(settings["skillOverrides"]["init"], "user-invocable-only");
    }

    /// Copies are explicit-only with their resources. Role skills, personal
    /// skills, tracked copies and project customizations are left alone. The
    /// worktree stays clean without hiding the main checkout's untracked files.
    #[test]
    fn grok_projection_copies_only_what_it_owns() {
        let dir = tempfile::tempdir().unwrap();
        let (home, main) = (dir.path().join("home"), dir.path().join("main"));
        let repo = main.join(".cas/worktrees/probe");
        for (name, body) in [
            ("cas-release-report", MANAGED),
            ("cas-worker", MANAGED),
            ("own-skill", "---\nname: own\n---\nPRIVATE\n"),
            ("customized", MANAGED),
            ("tracked", MANAGED),
        ] {
            std::fs::create_dir_all(home.join("skills").join(name).join("references")).unwrap();
            std::fs::write(home.join("skills").join(name).join("SKILL.md"), body).unwrap();
            std::fs::write(home.join("skills").join(name).join("references/a.md"), "A").unwrap();
        }
        std::fs::create_dir_all(main.join(GROK_PROJECT_SKILLS).join("tracked")).unwrap();
        std::fs::write(main.join(GROK_PROJECT_SKILLS).join("tracked/SKILL.md"), "tracked\n").unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["add", ".grok/skills/tracked/SKILL.md"]);
        git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "seed"]);
        git(&main, &["worktree", "add", "-q", "-b", "probe", ".cas/worktrees/probe"]);

        let project = repo.join(GROK_PROJECT_SKILLS);
        let customization = "---\nname: customized\n---\nMINE\n";
        for (name, body) in [("customized", customization), ("project-only", MANAGED)] {
            std::fs::create_dir_all(project.join(name)).unwrap();
            std::fs::write(project.join(name).join("SKILL.md"), body).unwrap();
        }
        let main_untracked = main.join(GROK_PROJECT_SKILLS).join("in-progress");
        std::fs::create_dir_all(&main_untracked).unwrap();
        std::fs::write(main_untracked.join("SKILL.md"), "draft\n").unwrap();

        assert_eq!(prepare_grok_project_skills(&repo, &home).unwrap(), 2);
        let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
        for name in ["cas-release-report", "project-only"] {
            let copy = read(project.join(name).join("SKILL.md"));
            assert!(copy.contains("\ndisable-model-invocation: true\n---\nBODY"), "{copy}");
        }
        assert_eq!(
            read(project.join("cas-release-report/references/a.md")),
            read(home.join("skills/cas-release-report/references/a.md"))
        );
        assert!(!project.join("cas-worker").exists() && !project.join("own-skill").exists());
        assert_eq!(read(project.join("customized/SKILL.md")), customization);
        // Clean status: the tracked copy is untouched and the rest ignores itself.
        assert!(git_lines(&repo, &["status", "--porcelain"]).is_empty());
        let main_status = ["status", "--porcelain", "--untracked-files=all", "--", ".grok/skills"];
        assert!(!git_lines(&main, &main_status).is_empty(), "the main checkout's draft stays visible");
        // A later launch refreshes in place rather than stacking frontmatter.
        assert_eq!(prepare_grok_project_skills(&repo, &home).unwrap(), 2);
        let again = read(project.join("project-only/SKILL.md"));
        assert_eq!(again.matches("disable-model-invocation").count(), 1);
    }
}
