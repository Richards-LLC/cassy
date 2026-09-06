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

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

/// Flavor subdirectories under `cas-cli/src/builtins` ("" = claude baseline).
const FLAVORS: [(&str, builtin_catalog::Flavor); 3] = [
    ("claude", builtin_catalog::Flavor::Claude),
    ("codex", builtin_catalog::Flavor::Codex),
    ("grok", builtin_catalog::Flavor::Grok),
];

/// Skills whose description, frontmatter and portability this task owns.
const OWNED_SKILLS: [&str; 10] = [
    "cas-dataviz",
    "cas-technical-drawing",
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

/// Every file (skill body or reference) under one owned skill, from the
/// embedded catalog rather than the absent archive checkout.
fn owned_skill_files() -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    for (label, flavor) in FLAVORS {
        for skill in OWNED_SKILLS {
            let prefix = format!("skills/{skill}/");
            for builtin in builtin_catalog::skills(flavor) {
                if builtin.path.starts_with(&prefix) {
                    found.push((format!("{label}/{}", builtin.path), builtin.content));
                }
            }
        }
    }
    found.sort();
    found
}

/// Every shipped skill body or top-level legacy flat skill path for one flavor.
fn skill_files(flavor: builtin_catalog::Flavor, label: &str) -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    for builtin in builtin_catalog::skills(flavor) {
        let Some(relative) = builtin.path.strip_prefix("skills/") else {
            continue;
        };
        if relative.ends_with("/SKILL.md") || !relative.contains('/') {
            found.push((format!("{label}/{}", builtin.path), builtin.content));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

// ---------------------------------------------------------------------------
// Descriptions
// ---------------------------------------------------------------------------

/// A description longer than the harness limit loses its trigger clause, and an
/// empty one gives the model nothing to route on.
#[test]
fn every_builtin_skill_description_is_present_and_within_the_harness_limit() {
    let mut problems = Vec::new();
    for (label, flavor) in FLAVORS {
        for (path, content) in skill_files(flavor, label) {
            match field(&content, "description") {
                None => problems.push(format!("{path}: no description field")),
                Some(description) if description.is_empty() => {
                    problems.push(format!("{path}: empty description"))
                }
                Some(description) if description.chars().count() > DESCRIPTION_MAX_CHARS => {
                    problems.push(format!(
                        "{path}: description is {} chars, over the {DESCRIPTION_MAX_CHARS} limit",
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
    for (label, flavor) in FLAVORS {
        for skill in OWNED_SKILLS {
            let content = builtin_catalog::find(flavor, &format!("skills/{skill}/SKILL.md"));
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
    for (label, flavor) in FLAVORS {
        let content = builtin_catalog::find(flavor, "skills/cas-dataviz/SKILL.md");
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
    for (path, content) in owned_skill_files() {
        for (needle, why) in BANNED {
            for (index, line) in content.lines().enumerate() {
                if line.contains(needle) {
                    problems.push(format!("{}:{}: {needle:?} ({why})", path, index + 1));
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
    for (label, flavor) in FLAVORS {
        for rel_path in [
            "skills/cli-routing/SKILL.md",
            "skills/cli-routing/references/routing.md",
        ] {
            let content = builtin_catalog::find(flavor, rel_path);
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
    for (label, flavor) in FLAVORS {
        for skill in ["cas-brainstorm", "cas-ideate"] {
            let content = builtin_catalog::find(flavor, &format!("skills/{skill}/SKILL.md"));
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
    for (label, flavor) in FLAVORS {
        let content = builtin_catalog::find(flavor, "skills/cas-nuxt-playwright/SKILL.md");
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
    for (path, content) in owned_skill_files() {
        for (index, line) in content.lines().enumerate() {
            // `/plan` as a command, not as part of `supervisor/planner`.
            if line.match_indices("/plan").any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + "/plan".len()..].chars().next();
                before.is_none_or(|c| !c.is_ascii_alphanumeric())
                    && after.is_none_or(|c| !c.is_ascii_alphabetic())
            }) {
                problems.push(format!("{path}:{}: {}", index + 1, line.trim()));
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
    for (_, flavor) in FLAVORS {
        for skill in ["cas-brainstorm", "cas-ideate"] {
            let prefix = format!("skills/{skill}/");
            let count = builtin_catalog::skills(flavor)
                .iter()
                .filter(|builtin| builtin.path.starts_with(&prefix))
                .map(|builtin| builtin.content.matches(NEEDLE).count())
                .sum::<usize>();
            if count > 1 {
                problems.push(format!(
                    "{}/{skill}: {NEEDLE:?} stated {count}x",
                    if flavor == builtin_catalog::Flavor::Claude {
                        "claude"
                    } else if flavor == builtin_catalog::Flavor::Codex {
                        "codex"
                    } else {
                        "grok"
                    },
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
        .set(
            "release.claude_account_allowlist",
            "ops@example.com, Release@Example.com",
        )
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
