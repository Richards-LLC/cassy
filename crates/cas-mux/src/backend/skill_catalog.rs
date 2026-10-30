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
/// `/name` working. Only Cassy-managed skills are copied; a user's own personal
/// skills stay listed, because their contents are never read.
pub(super) fn prepare_grok_project_skills(cwd: &Path, grok_home: &Path) -> std::io::Result<usize> {
    let source = grok_home.join("skills");
    let Ok(entries) = std::fs::read_dir(&source) else {
        return Ok(0);
    };
    let target = cwd.join(GROK_PROJECT_SKILLS);
    let tracked = git_lines(cwd, &["ls-files", "--", GROK_PROJECT_SKILLS]);
    let mut projected = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| !is_role_skill(name)) else {
            continue;
        };
        let skill = entry.path();
        let Ok(body) = std::fs::read_to_string(skill.join("SKILL.md")) else {
            continue;
        };
        let destination = target.join(name);
        let prefix = format!("{GROK_PROJECT_SKILLS}/{name}/");
        let customized = std::fs::read_to_string(destination.join("SKILL.md"))
            .is_ok_and(|existing| !is_cassy_managed(&existing));
        if !is_cassy_managed(&body) || customized || tracked.iter().any(|file| file.starts_with(&prefix)) {
            continue;
        }
        copy_tree(&skill, &destination)?;
        std::fs::write(destination.join("SKILL.md"), explicit_only(&body))?;
        projected += 1;
    }
    if projected > 0 {
        exclude_from_git(cwd)?;
    }
    Ok(projected)
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

/// Keep the projection out of `git status` without editing a tracked file.
fn exclude_from_git(cwd: &Path) -> std::io::Result<()> {
    let probe = format!("{GROK_PROJECT_SKILLS}/probe/SKILL.md");
    if !git_lines(cwd, &["check-ignore", &probe]).is_empty() {
        return Ok(());
    }
    let Some(exclude) = git_lines(cwd, &["rev-parse", "--git-path", "info/exclude"]).pop() else {
        return Ok(());
    };
    let exclude = cwd.join(PathBuf::from(exclude));
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("/{GROK_PROJECT_SKILLS}/\n"));
    std::fs::write(exclude, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WorkerLaunchConfig;
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

        let grok = worker_config(SupervisorCli::Grok, dir.path(), &account);
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

    /// Copies are explicit-only with their resources; role skills, personal
    /// skills, tracked copies and project customizations are left alone, and
    /// the checkout stays clean.
    #[test]
    fn grok_projection_copies_only_what_it_owns() {
        let dir = tempfile::tempdir().unwrap();
        let (home, repo) = (dir.path().join("home"), dir.path().join("repo"));
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
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
        let project = repo.join(GROK_PROJECT_SKILLS);
        std::fs::create_dir_all(project.join("customized")).unwrap();
        let customization = "---\nname: customized\n---\nMINE\n";
        std::fs::write(project.join("customized/SKILL.md"), customization).unwrap();
        std::fs::create_dir_all(project.join("tracked")).unwrap();
        std::fs::write(project.join("tracked/SKILL.md"), "tracked\n").unwrap();
        git(&repo, &["add", ".grok/skills/tracked/SKILL.md"]);
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "seed"]);

        assert_eq!(prepare_grok_project_skills(&repo, &home).unwrap(), 1);
        let copy = std::fs::read_to_string(project.join("cas-release-report/SKILL.md")).unwrap();
        assert!(copy.contains("\ndisable-model-invocation: true\n---\nBODY"), "{copy}");
        let read = |path: PathBuf| std::fs::read_to_string(path).unwrap();
        assert_eq!(read(project.join("cas-release-report/references/a.md")), read(home.join("skills/cas-release-report/references/a.md")));
        assert!(!project.join("cas-worker").exists() && !project.join("own-skill").exists());
        assert_eq!(read(project.join("customized/SKILL.md")), customization);
        // Clean status: the tracked copy is untouched and the rest is excluded.
        assert!(git_lines(&repo, &["status", "--porcelain"]).is_empty());
        // A later launch refreshes in place rather than stacking frontmatter.
        assert_eq!(prepare_grok_project_skills(&repo, &home).unwrap(), 1);
        let again = std::fs::read_to_string(project.join("cas-release-report/SKILL.md")).unwrap();
        assert_eq!(again.matches("disable-model-invocation").count(), 1);
    }
}
