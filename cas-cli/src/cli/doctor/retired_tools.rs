//! Read-only guidance inventory against effective MCP registrations (cas-1ad4d).

use super::{Check, CheckStatus};
use anyhow::{Context, Result};
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const NAME: &str = "retired MCP references";

pub(super) fn check(cas_root: &Path) -> Check {
    match scan(cas_root) {
        Ok(findings) if findings.is_empty() => Check::new(
            NAME,
            CheckStatus::Ok,
            "no explicit references to unavailable MCP servers",
        ),
        Ok(findings) => Check::new(
            NAME,
            CheckStatus::Warning,
            format!(
                "{} guidance reference(s) name absent or disabled MCP servers (review mentions, including historical or retirement instructions):\n{}\nReview the named guidance or restore its registration; doctor does not edit it.",
                findings.len(),
                findings.into_iter().collect::<Vec<_>>().join("\n"),
            ),
        ),
        // Never silently turn an incomplete inventory into a clean bill of health.
        Err(error) => Check::new(
            NAME,
            CheckStatus::Warning,
            format!("MCP guidance scan incomplete: {error:#}"),
        ),
    }
}

fn scan(cas_root: &Path) -> Result<BTreeSet<String>> {
    let project = cas_root.parent().context("project root unavailable")?;
    let home = dirs::home_dir().context("home directory unavailable")?;
    let proxy = cas_root.join("proxy.toml");
    let (proxy_config, _) = cmcp_core::config::Config::load_merged_with_sources(
        proxy.exists().then_some(proxy.as_path()),
    ).map_err(|_| anyhow::anyhow!("cannot read merged MCP proxy registrations; review proxy.toml and the user proxy config"))?;
    // Cassy's own tool namespace is supplied by `cas serve`, not its upstream proxy.
    let mut available: BTreeSet<String> = ["cas", "cs", "cassy"]
        .into_iter()
        .map(str::to_string)
        .collect();
    available.extend(
        proxy_config
            .servers
            .keys()
            .map(|name| name.to_ascii_lowercase()),
    );
    for (user, local, table) in [
        (
            home.join(".claude.json"),
            project.join(".mcp.json"),
            "mcpServers",
        ),
        (
            home.join(".codex/config.toml"),
            project.join(".codex/config.toml"),
            "mcp_servers",
        ),
        (
            home.join(".grok/settings.json"),
            project.join(".grok/settings.json"),
            "mcpServers",
        ),
    ] {
        let mut registrations = registrations(&user, table)?;
        registrations.extend(self::registrations(&local, table)?);
        available.extend(
            registrations
                .into_iter()
                .filter_map(|(name, enabled)| enabled.then_some(name)),
        );
    }

    let mut documents = BTreeMap::new();
    let mut visited = BTreeSet::new();
    for root in [
        Some(cas_root.to_path_buf()),
        crate::config::global_cas_dir(),
    ]
    .into_iter()
    .flatten()
    {
        scan_database(&root.join("cas.db"), &mut documents)?;
        for relative in ["entries", "rules", "templates", "briefs"] {
            scan_files(&root.join(relative), &mut documents, &mut visited)?;
        }
    }
    for base in [project, home.as_path()] {
        for relative in [
            ".claude/rules",
            ".claude/skills",
            ".claude/memory",
            ".codex/skills",
            ".grok/skills",
            ".cas/templates",
            ".cas/briefs",
        ] {
            scan_files(&base.join(relative), &mut documents, &mut visited)?;
        }
    }
    for (directory, _) in super::user_skill_scan_targets() {
        scan_files(&directory, &mut documents, &mut visited)?;
    }
    // Builtin skill references and factory brief bodies are compiled into the binary.
    // Deduplicate the three harness projections by canonical source path and body.
    for file in crate::builtins::BUILTIN_SKILLS
        .iter()
        .chain(crate::builtins::CODEX_BUILTIN_SKILLS)
        .chain(crate::builtins::GROK_BUILTIN_SKILLS)
        .chain(crate::builtins::BUILTIN_AGENTS)
    {
        documents
            .entry(format!("builtin:{}", file.path))
            .or_insert_with(|| file.content.to_string());
    }
    documents.insert(
        "builtin:worker-guidance".into(),
        crate::builtins::worker_guidance(),
    );
    documents.insert(
        "builtin:supervisor-guidance".into(),
        crate::builtins::supervisor_guidance(),
    );

    // Only explicit MCP spellings establish a server name: ordinary Neon/Vercel
    // CLI guidance must not be diagnosed as an obsolete MCP dependency.
    let patterns = [
        Regex::new(r"(?i)\bmcp__([a-z0-9][a-z0-9_-]*?)__\w+")?,
        Regex::new(r"\b([A-Z][a-zA-Z0-9_-]*)[ `]*\s+(?i:MCP)\b")?,
        Regex::new(r"(?i)\b(neon|vercel)[ `]*\s+MCP\b")?,
        Regex::new(r#"(?i)\bMCP\s+server\s+[`"']([a-z0-9][a-z0-9_-]*)[`"']"#)?,
    ];
    let mut findings = BTreeSet::new();
    for (source, body) in documents {
        for (index, line) in body.lines().enumerate() {
            for (pattern_index, pattern) in patterns.iter().enumerate() {
                for capture in pattern.captures_iter(line) {
                    let server = capture[1].to_ascii_lowercase();
                    // These are prose qualifiers, not server names.
                    if pattern_index == 1
                        && [
                            "the",
                            "a",
                            "an",
                            "use",
                            "any",
                            "all",
                            "no",
                            "external",
                            "upstream",
                            "retired",
                            "removed",
                            "disabled",
                            "configured",
                            "existing",
                            "own",
                            "native",
                            "local",
                            "remote",
                            "direct",
                            "every",
                            "each",
                            "server",
                            "http",
                            "mcp",
                            "manage",
                            "when",
                            "opencode",
                            "codex",
                            "claude",
                        ]
                        .contains(&server.as_str())
                    {
                        continue;
                    }
                    if !available.contains(&server) {
                        findings.insert(format!(
                            "  {source}:{} — {server} (absent or disabled)",
                            index + 1
                        ));
                    }
                }
            }
        }
    }
    Ok(findings)
}

/// Read only registration names and enable switches, never output credentials.
fn registrations(path: &Path, table: &str) -> Result<BTreeMap<String, bool>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let value: serde_json::Value = if path.extension().is_some_and(|ext| ext == "toml") {
        let document: toml::Value = toml::from_str(&content)
            .map_err(|_| anyhow::anyhow!("invalid TOML in {}", path.display()))?;
        serde_json::to_value(document)?
    } else {
        serde_json::from_str(&content)
            .map_err(|_| anyhow::anyhow!("invalid JSON in {}", path.display()))?
    };
    let Some(servers) = value.get(table) else {
        return Ok(BTreeMap::new());
    };
    let servers = servers
        .as_object()
        .with_context(|| format!("invalid {table} table in {}", path.display()))?;
    Ok(servers
        .iter()
        .map(|(name, entry)| {
            (
                name.to_ascii_lowercase(),
                entry.get("disabled").and_then(|v| v.as_bool()) != Some(true)
                    && entry.get("enabled").and_then(|v| v.as_bool()) != Some(false),
            )
        })
        .collect())
}

fn scan_database(path: &Path, documents: &mut BTreeMap<String, String>) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open {}", path.display()))?;
    for (table, kind, wanted) in [
        ("rules", "rule", &["content", "auto_approve_tools"][..]),
        (
            "entries",
            "memory",
            &["title", "content", "raw_content"][..],
        ),
        (
            "skills",
            "skill",
            &[
                "name",
                "description",
                "invocation",
                "example",
                "summary",
                "preconditions",
                "postconditions",
                "validation_script",
                "allowed_tools",
                "disallowed_tools",
            ][..],
        ),
    ] {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        // Skills are initialized lazily. Older schemas may lack optional text fields.
        if columns.is_empty() {
            continue;
        }
        let selected = wanted
            .iter()
            .filter(|name| columns.contains(**name))
            .map(|name| format!("coalesce({name}, '')"))
            .collect::<Vec<_>>()
            .join(" || char(10) || ");
        let invocation = if table == "skills" && columns.contains("invocation") {
            "coalesce(invocation, '')"
        } else {
            "''"
        };
        let skill_type = if table == "skills" && columns.contains("skill_type") {
            "coalesce(skill_type, '')"
        } else {
            "''"
        };
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, {selected}, {invocation}, {skill_type} FROM {table}"
            ))
            .with_context(|| format!("cannot read {table} in {}", path.display()))?;
        for row in stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })? {
            let (id, mut body, invocation, skill_type) = row?;
            if skill_type == "mcp" {
                if let Ok(route) =
                    cmcp_core::config::ExternalToolConfig::parse_allowlist_entry(&invocation)
                {
                    if route.server != "*" {
                        body.push_str(&format!("\nMCP server `{}`", route.server));
                    }
                }
            }
            documents.insert(format!("{} [{kind} {id}]", path.display()), body);
        }
    }
    Ok(())
}

fn scan_files(
    path: &Path,
    documents: &mut BTreeMap<String, String>,
    visited: &mut BTreeSet<std::path::PathBuf>,
) -> Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    // Installed skills may be symlinks shared by harnesses. Canonical identity
    // prevents duplicate hits and cycles while retaining their real file location.
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))?;
    if !visited.insert(canonical.clone()) {
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(path).with_context(|| format!("cannot scan {}", path.display()))?
        {
            scan_files(&entry?.path(), documents, visited)?;
        }
    } else if metadata.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext == "md" || ext == "txt")
    {
        anyhow::ensure!(
            metadata.len() <= 1024 * 1024,
            "guidance file too large to scan: {}",
            path.display()
        );
        documents.insert(
            canonical.display().to_string(),
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        init_cas_dir, open_rule_store_local, open_skill_store_local, open_store_local,
    };
    use crate::types::{Entry, Rule, Skill, SkillType};
    use std::fs;

    #[test]
    fn doctor_lists_removed_mcp_references_by_id_and_file_cas_1ad4d() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("project");
        fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        open_rule_store_local(&root)
            .unwrap()
            .add(&Rule::new(
                "rule-removed".into(),
                "Use the Neon MCP run_sql tool".into(),
            ))
            .unwrap();
        open_store_local(&root)
            .unwrap()
            .add(&Entry::new(
                "mem-removed".into(),
                "Call mcp__neon__run_sql".into(),
            ))
            .unwrap();
        let mut skill = Skill::new("skill-removed".into(), "old database tool".into());
        skill.skill_type = SkillType::Mcp;
        skill.invocation = "neon.run_sql".into();
        open_skill_store_local(&root).unwrap().add(&skill).unwrap();
        let brief = root.join("templates/worker-brief.md");
        fs::create_dir_all(brief.parent().unwrap()).unwrap();
        fs::write(&brief, "Use the Vercel MCP for deployment").unwrap();
        let row = check(&root);
        assert!(
            matches!(row.status, CheckStatus::Warning),
            "{}",
            row.message
        );
        for source in [
            "rule-removed",
            "mem-removed",
            "skill-removed",
            "templates/worker-brief.md",
        ] {
            assert!(
                row.message.contains(source),
                "missing {source}: {}",
                row.message
            );
        }

        // Registration restores availability without editing the guidance.
        fs::write(root.join("proxy.toml"), "[servers.neon]\ntransport = 'http'\nurl = 'https://example.test/neon'\n[servers.vercel]\ntransport = 'http'\nurl = 'https://example.test/vercel'\n").unwrap();
        let available = check(&root);
        assert!(
            !available.message.contains("rule-removed")
                && !available.message.contains("mem-removed")
                && !available.message.contains("skill-removed")
                && !available.message.contains("templates/worker-brief.md"),
            "{}",
            available.message
        );
    }

    #[test]
    fn doctor_honors_disabled_registrations_and_reports_incomplete_scans() {
        let env = crate::test_support::TestEnvGuard::temp_home();
        let project = env.home().join("project");
        fs::create_dir_all(&project).unwrap();
        let root = init_cas_dir(&project).unwrap();
        let rules = project.join(".claude/rules");
        fs::create_dir_all(&rules).unwrap();
        let guidance = rules.join("database.md");
        let retired_server = "acme";
        let body = format!("Use mcp__{retired_server}__run_sql\nUse the Neon CLI and Vercel CLI.\nUse any available MCP server.\nCall mcp__cs__task.\n");
        fs::write(&guidance, &body).unwrap();
        fs::write(
            env.home().join(".claude.json"),
            serde_json::json!({"mcpServers": {(retired_server): {"command": "fixture"}}}).to_string(),
        )
        .unwrap();
        assert!(
            !check(&root).message.contains("database.md"),
            "an enabled user registration is available"
        );
        fs::write(
            project.join(".mcp.json"),
            serde_json::json!({"mcpServers": {(retired_server): {"disabled": true}}}).to_string(),
        )
        .unwrap();
        let row = check(&root);
        assert!(matches!(row.status, CheckStatus::Warning));
        assert!(
            row.message.contains("database.md:1") && row.message.contains(retired_server),
            "{}",
            row.message
        );
        assert!(
            !row.message.contains("database.md:2")
                && !row.message.contains("database.md:3")
                && !row.message.contains("database.md:4"),
            "CLI and generic prose are not MCP dependencies: {}",
            row.message
        );
        assert_eq!(
            fs::read_to_string(&guidance).unwrap(),
            body,
            "doctor must preserve guidance"
        );
        let json = super::super::serialize_checks(&[row], &[]);
        assert_eq!(json[0]["group"], "config");
        assert!(
            json[0]["message"]
                .as_str()
                .unwrap()
                .contains("database.md:1")
        );

        let global = crate::config::global_cas_dir().unwrap();
        fs::create_dir_all(&global).unwrap();
        open_rule_store_local(&global)
            .unwrap()
            .add(&Rule::new(
                "rule-global-removed".into(),
                "Use MCP server `oldvendor`".into(),
            ))
            .unwrap();
        assert!(check(&root).message.contains("rule-global-removed"));
        #[cfg(unix)]
        {
            let skills = env.home().join("shared-skills");
            fs::create_dir_all(&skills).unwrap();
            fs::write(skills.join("SKILL.md"), "Use mcp__oldvendor__read").unwrap();
            std::os::unix::fs::symlink(&skills, skills.join("cycle")).unwrap();
            std::os::unix::fs::symlink(&skills, project.join(".claude/skills")).unwrap();
            let row = check(&root);
            assert_eq!(
                row.message.matches("shared-skills/SKILL.md:1").count(),
                1,
                "{}",
                row.message
            );
        }

        fs::write(
            project.join(".mcp.json"),
            "invalid JSON secret-fixture-token",
        )
        .unwrap();
        let incomplete = check(&root);
        assert!(matches!(incomplete.status, CheckStatus::Warning));

        assert!(!incomplete.message.contains("secret-fixture-token"));
    }
}
