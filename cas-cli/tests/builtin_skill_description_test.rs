//! Description, portability and frontmatter guards for shipped built-in skills
//! (cas-37f6).
//!
//! Three separate defects motivated this file:
//!
//!   1. Descriptions that do not lead with a trigger, or that collide with a
//!      harness-bundled skill of the same name, so the model cannot route.
//!   2. Operator-specific facts baked into files `cas init` installs into every
//!      project: an e-mail address, `/home/<operator>/...` paths, links of the
//!      form `../../../../../docs/...` that only resolve inside this source
//!      tree, and `.cas/artifacts/...` research links that resolve nowhere.
//!   3. Frontmatter that contradicts the skill body — `disallowed-tools: Write`
//!      on skills whose own procedure writes a document.
//!
//! Everything here is a repo-local filesystem read; no network, no builds.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    cas::test_paths::workspace_root()
}

fn builtins_root() -> PathBuf {
    repo_root().join("cas-cli/src/builtins")
}

/// Flavor subdirectories under `cas-cli/src/builtins` ("" = claude baseline).
const FLAVORS: [(&str, &str); 3] = [("claude", ""), ("codex", "codex"), ("grok", "grok")];

/// Skills whose description, frontmatter and portability this task owns.
const OWNED_SKILLS: [&str; 9] = [
    "cas-dataviz",
    "cas-image-generate",
    "cli-routing",
    "cas-nuxt-playwright",
    "cas-tdd",
    "cas-wizard",
    "session-learn",
    "cas-brainstorm",
    "cas-ideate",
];

/// Claude Code truncates a skill description past this length; a truncated
/// description silently loses its trigger clause.
const DESCRIPTION_MAX_CHARS: usize = 1024;

fn flavor_dir(subdir: &str) -> PathBuf {
    if subdir.is_empty() {
        builtins_root()
    } else {
        builtins_root().join(subdir)
    }
}

/// Every `SKILL.md` under a flavor's `skills/` tree, relative to that tree.
fn skill_files(flavor_subdir: &str) -> Vec<PathBuf> {
    let root = flavor_dir(flavor_subdir).join("skills");
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Twin trees live under the claude root; do not walk into them.
                if flavor_subdir.is_empty()
                    && path.file_name().is_some_and(|n| n == "codex" || n == "grok")
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md")
                || path.extension().and_then(|e| e.to_str()) == Some("md")
                    && path.parent() == Some(root.as_path())
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Extract the YAML frontmatter block of a markdown file, if present.
fn frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// Value of a single-line `key: value` frontmatter field.
fn field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let block = frontmatter(content)?;
    block.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
            .map(str::trim)
    })
}

/// Every markdown file (skill body or reference) under one owned skill.
fn owned_skill_files() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (_, subdir) in FLAVORS {
        for skill in OWNED_SKILLS {
            let dir = flavor_dir(subdir).join("skills").join(skill);
            let mut stack = vec![dir];
            while let Some(current) = stack.pop() {
                let Ok(entries) = fs::read_dir(&current) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        found.push(path);
                    }
                }
            }
        }
    }
    found.sort();
    found
}

fn rel(path: &Path) -> String {
    path.strip_prefix(builtins_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

// ---------------------------------------------------------------------------
// Descriptions
// ---------------------------------------------------------------------------

/// A description longer than the harness limit loses its trigger clause, and an
/// empty one gives the model nothing to route on.
#[test]
fn every_builtin_skill_description_is_present_and_within_the_harness_limit() {
    let mut problems = Vec::new();
    for (label, subdir) in FLAVORS {
        for path in skill_files(subdir) {
            let content = fs::read_to_string(&path).expect("skill body");
            match field(&content, "description") {
                None => problems.push(format!("{label} {}: no description field", rel(&path))),
                Some(description) if description.is_empty() => {
                    problems.push(format!("{label} {}: empty description", rel(&path)))
                }
                Some(description) if description.chars().count() > DESCRIPTION_MAX_CHARS => {
                    problems.push(format!(
                        "{label} {}: description is {} chars, over the {DESCRIPTION_MAX_CHARS} limit",
                        rel(&path),
                        description.chars().count()
                    ))
                }
                Some(_) => {}
            }
        }
    }
    assert!(problems.is_empty(), "\n  {}\n", problems.join("\n  "));
}

/// The routing convention: the description opens with the trigger, not with an
/// identity sentence, a provider name, or a shouted opt-in.
#[test]
fn owned_skill_descriptions_lead_with_a_use_when_trigger() {
    let mut problems = Vec::new();
    for (label, subdir) in FLAVORS {
        for skill in OWNED_SKILLS {
            let path = flavor_dir(subdir)
                .join("skills")
                .join(skill)
                .join("SKILL.md");
            let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{skill}: {e}"));
            let description = field(&content, "description")
                .unwrap_or_else(|| panic!("{label} {skill} has no description"));
            if !description.starts_with("Use when ") {
                problems.push(format!(
                    "{label} {skill}: description must open with \"Use when \", got {description:?}"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "\n  {}\n", problems.join("\n  "));
}

/// cas-dataviz previously carried the bundled `dataviz` skill's trigger list
/// word for word, so both skills matched the same prompts.
#[test]
fn cas_dataviz_description_names_its_boundary_with_the_bundled_skill() {
    for (label, subdir) in FLAVORS {
        let path = flavor_dir(subdir)
            .join("skills/cas-dataviz/SKILL.md")
            .to_path_buf();
        let content = fs::read_to_string(&path).expect("cas-dataviz body");
        let description = field(&content, "description").expect("cas-dataviz description");
        for required in ["static", "SVG", "bundled"] {
            assert!(
                description.contains(required),
                "{label} cas-dataviz description must name {required:?}: {description:?}"
            );
        }
        assert!(
            !description.contains("dashboard"),
            "{label} cas-dataviz description still claims the bundled skill's dashboard trigger"
        );
    }
}

// ---------------------------------------------------------------------------
// Portability of shipped text
// ---------------------------------------------------------------------------

/// Operator-specific and source-tree-only facts must not ship inside skills
/// that `cas init` installs into unrelated projects.
#[test]
fn owned_skills_carry_no_operator_or_source_tree_specific_text() {
    // (needle, why it cannot ship)
    const BANNED: [(&str, &str); 5] = [
        ("pippenz", "operator account/home directory"),
        ("/home/", "absolute operator path"),
        (
            "SLACK_POSTING_RUNBOOK",
            "runbook that is not shipped with the skill",
        ),
        (
            "../../../../../",
            "link that only resolves inside this source tree",
        ),
        (".cas/artifacts", "link into a local artifacts directory"),
    ];

    let mut problems = Vec::new();
    for path in owned_skill_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (needle, why) in BANNED {
            for (index, line) in content.lines().enumerate() {
                if line.contains(needle) {
                    problems.push(format!(
                        "{}:{}: {needle:?} ({why})",
                        rel(&path),
                        index + 1
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "\nShipped built-in skills must be portable:\n  {}\n",
        problems.join("\n  ")
    );
}

/// The Claude account gate is operator policy, so it belongs in configuration
/// that a project sets, not in the shipped skill text.
#[test]
fn cli_routing_expresses_the_account_gate_as_a_config_key() {
    for (label, subdir) in FLAVORS {
        for rel_path in [
            "skills/cli-routing/SKILL.md",
            "skills/cli-routing/references/routing.md",
        ] {
            let path = flavor_dir(subdir).join(rel_path);
            let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel_path}: {e}"));
            assert!(
                content.contains("release.claude_account_allowlist"),
                "{label} {rel_path} must name the release.claude_account_allowlist config key"
            );
            for token in content.split_whitespace().filter(|t| t.contains('@')) {
                let address = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
                assert!(
                    address.ends_with("example.com"),
                    "{label} {rel_path} carries a real e-mail address ({address:?}); \
                     the allowlist belongs in configuration, and examples use example.com"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Frontmatter that contradicts the body
// ---------------------------------------------------------------------------

/// cas-brainstorm and cas-ideate both write a document as their final phase;
/// `disallowed-tools: Write, Edit` made that phase impossible.
#[test]
fn artifact_writing_skills_do_not_disallow_write_and_edit() {
    for (label, subdir) in FLAVORS {
        for skill in ["cas-brainstorm", "cas-ideate"] {
            let path = flavor_dir(subdir)
                .join("skills")
                .join(skill)
                .join("SKILL.md");
            let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{skill}: {e}"));
            let block = frontmatter(&content).expect("frontmatter");
            assert!(
                !block.contains("disallowed-tools"),
                "{label} {skill} disallows tools its own artifact phase requires"
            );
        }
    }
}

/// A stack-specific skill opts out of model invocation through frontmatter, not
/// by shouting the opt-in inside the description.
#[test]
fn cas_nuxt_playwright_opts_out_through_frontmatter() {
    for (label, subdir) in FLAVORS {
        let path = flavor_dir(subdir).join("skills/cas-nuxt-playwright/SKILL.md");
        let content = fs::read_to_string(&path).expect("cas-nuxt-playwright body");
        let block = frontmatter(&content).expect("frontmatter");
        assert!(
            block.contains("disable-model-invocation: true"),
            "{label} cas-nuxt-playwright must set disable-model-invocation: true"
        );
        assert!(
            !block.contains("user-invocable:"),
            "{label} cas-nuxt-playwright: user-invocable is the default and is redundant here"
        );
        let description = field(&content, "description").expect("description");
        assert!(
            description.contains("Nuxt") && description.contains("Playwright"),
            "{label} cas-nuxt-playwright description must name its stack: {description:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Handoffs and repeated harness-enforced prose
// ---------------------------------------------------------------------------

/// `/plan` is not a skill or a command in any tree; the real handoff for
/// planning is cas-supervisor epic creation.
#[test]
fn brainstorm_hands_off_to_cas_supervisor_not_a_nonexistent_plan_command() {
    let mut problems = Vec::new();
    for path in owned_skill_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            // `/plan` as a command, not as part of `supervisor/planner`.
            if line.match_indices("/plan").any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + "/plan".len()..].chars().next();
                before.is_none_or(|c| !c.is_ascii_alphanumeric())
                    && after.is_none_or(|c| !c.is_ascii_alphabetic())
            }) {
                problems.push(format!("{}:{}: {}", rel(&path), index + 1, line.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "\nHand off to cas-supervisor; /plan does not exist:\n  {}\n",
        problems.join("\n  ")
    );
}

/// The PreToolUse guard already denies AskUserQuestion in factory mode and
/// returns that guidance; repeating it in prose costs context on every load.
#[test]
fn askuserquestion_fallback_is_stated_at_most_once_per_skill() {
    const NEEDLE: &str = "AskUserQuestion is blocked";
    let mut problems = Vec::new();
    for (_, subdir) in FLAVORS {
        for skill in ["cas-brainstorm", "cas-ideate"] {
            let dir = flavor_dir(subdir).join("skills").join(skill);
            let mut count = 0usize;
            let mut stack = vec![dir];
            while let Some(current) = stack.pop() {
                let Ok(entries) = fs::read_dir(&current) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    let Ok(content) = fs::read_to_string(&path) else {
                        continue;
                    };
                    count += content.matches(NEEDLE).count();
                }
            }
            if count > 1 {
                problems.push(format!(
                    "{}/{skill}: {NEEDLE:?} stated {count}x",
                    if subdir.is_empty() { "claude" } else { subdir },
                ));
            }
        }
    }
    assert!(problems.is_empty(), "\n  {}\n", problems.join("\n  "));
}

// ---------------------------------------------------------------------------
// The config key the skill now points at
// ---------------------------------------------------------------------------

/// The allowlist is empty by default and the gate fails closed, so an
/// unconfigured project approves no Claude account at all.
#[test]
fn claude_account_allowlist_defaults_to_denying_every_account() {
    let config = cas::config::Config::default();
    assert!(
        config.release.is_none(),
        "[release] must be absent until a project configures it"
    );
    assert!(
        !config.claude_account_allowed("anyone@example.com"),
        "an unconfigured allowlist must approve nobody"
    );
}

/// Once configured, the gate is an exact membership test on the probed e-mail.
#[test]
fn claude_account_allowlist_is_an_exact_membership_test() {
    let mut config = cas::config::Config::default();
    config
        .set("release.claude_account_allowlist", "ops@example.com, Release@Example.com")
        .expect("set the allowlist");

    assert!(config.claude_account_allowed("ops@example.com"));
    assert!(
        config.claude_account_allowed("release@example.com"),
        "e-mail comparison is case-insensitive"
    );
    assert!(
        config.claude_account_allowed("  ops@example.com  "),
        "a probed value is trimmed before comparison"
    );
    assert!(!config.claude_account_allowed("other@example.com"));
    assert!(!config.claude_account_allowed(""));

    assert_eq!(
        config.get("release.claude_account_allowlist"),
        Some("ops@example.com,release@example.com".to_string()),
        "the stored allowlist round-trips through config get"
    );
}

/// An operator cannot configure a key that `cas config` does not document.
#[test]
fn claude_account_allowlist_is_registered_and_documented() {
    let registry = cas::config::ConfigRegistry::new();
    let meta = registry
        .get("release.claude_account_allowlist")
        .expect("release.claude_account_allowlist must be registered");
    assert_eq!(meta.section, "release");
    assert_eq!(meta.value_type, cas::config::ConfigType::StringList);
    assert_eq!(meta.default, "");
    assert!(
        registry.sections().contains(&"release"),
        "the [release] section must be listed by `cas config`"
    );
}
