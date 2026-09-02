//! Keep action lists in the built-in skills aligned with the live MCP dispatch.
//!
//! The service dispatch is intentionally the source of truth here. These
//! tests parse its action arms and compare all three shipped skill flavors,
//! while the expected arrays pin the public contract against accidental
//! removal or reordering.

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

const TASK_ACTIONS: &[&str] = &[
    "create",
    "proposal_inbox",
    "proposal_accept",
    "proposal_reject",
    "proposal_reconcile",
    "show",
    "update",
    "start",
    "close",
    "cancel",
    "reopen",
    "request_changes",
    "delete",
    "list",
    "ready",
    "blocked",
    "notes",
    "dep_add",
    "dep_remove",
    "dep_list",
    "claim",
    "release",
    "reset",
    "transfer",
    "available",
    "mine",
];

const SEARCH_ACTIONS: &[&str] = &[
    "search",
    "retrieval_feedback",
    "retrieval_metrics",
    "skill_impact",
    "impact_report",
    "context",
    "context_for_subagent",
    "observe",
    "entity_list",
    "entity_show",
    "entity_extract",
    "code_search",
    "code_show",
    "grep",
    "blame",
    "history",
];

const MEMORY_ACTIONS: &[&str] = &[
    "remember",
    "get",
    "list",
    "update",
    "delete",
    "archive",
    "unarchive",
    "helpful",
    "harmful",
    "mark_reviewed",
    "recent",
    "set_tier",
    "opinion_reinforce",
    "opinion_weaken",
    "opinion_contradict",
];

const MEMORY_FIELDS: &[&str] = &[
    "action",
    "id",
    "content",
    "entry_type",
    "tags",
    "title",
    "importance",
    "tier",
    "limit",
    "scope",
    "team_id",
    "bypass_overlap",
    "mode",
    "expected_updated_at",
    "sort",
    "sort_order",
    "valid_from",
    "valid_until",
    "personal",
];

#[derive(Clone, Copy)]
struct Flavor {
    name: &'static str,
    catalog: builtin_catalog::Flavor,
}

const FLAVORS: &[Flavor] = &[
    Flavor {
        name: "claude",
        catalog: builtin_catalog::Flavor::Claude,
    },
    Flavor {
        name: "codex",
        catalog: builtin_catalog::Flavor::Codex,
    },
    Flavor {
        name: "grok",
        catalog: builtin_catalog::Flavor::Grok,
    },
];

fn service_source() -> &'static str {
    include_str!("../src/mcp/tools/service/mod.rs")
}

fn memory_request_source() -> &'static str {
    include_str!("../../crates/cas-mcp/src/types.rs")
}

fn function_section<'a>(source: &'a str, function: &str, next_marker: &str) -> &'a str {
    let start = source
        .find(&format!("pub async fn {function}("))
        .unwrap_or_else(|| panic!("missing {function} dispatch function"));
    let section = &source[start..];
    let end = section
        .find(next_marker)
        .unwrap_or_else(|| panic!("missing end marker for {function} dispatch function"));
    &section[..end]
}

fn dispatch_actions(section: &str) -> Vec<String> {
    let match_start = section
        .find("let result = match req.action.as_str() {")
        .expect("dispatch result match");
    let arms = &section[match_start..];
    let arms = arms
        .split_once('{')
        .expect("dispatch match opening brace")
        .1;

    arms.lines()
        .take_while(|line| !line.trim_start().starts_with("_ =>"))
        .filter_map(|line| line.split_once("=>").map(|(left, _)| left))
        .flat_map(|left| left.split('|'))
        .map(str::trim)
        .filter(|token| token.starts_with('"') && token.ends_with('"'))
        .map(|token| token.trim_matches('"').to_string())
        .collect()
}

fn section<'a>(content: &'a str, heading: &str) -> &'a str {
    let start = content
        .find(heading)
        .unwrap_or_else(|| panic!("missing {heading:?}"));
    let body = &content[start + heading.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    &body[..end]
}

fn documented_actions(content: &str) -> Vec<String> {
    let body = section(content, "## Valid Actions");
    let line = body
        .lines()
        .find(|line| line.contains("actions**"))
        .expect("canonical action list line");
    let list = line
        .split_once("): ")
        .map(|(_, list)| list)
        .expect("canonical action list delimiter");

    let mut actions = Vec::new();
    let mut parts = list.split('`');
    parts.next();
    while let Some(action) = parts.next() {
        actions.push(action.to_string());
        parts.next();
    }
    actions
}

fn documented_memory_fields(content: &str) -> Vec<String> {
    section(content, "## Request Fields")
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- `"))
        .filter_map(|field| field.split('`').next())
        .map(str::to_string)
        .collect()
}

fn memory_request_fields(source: &str) -> Vec<String> {
    let body = source
        .split_once("pub struct MemoryRequest {")
        .expect("MemoryRequest definition")
        .1
        .split_once("\n}")
        .expect("MemoryRequest closing brace")
        .0;

    body.lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub "))
        .filter_map(|field| field.split(':').next())
        .map(str::to_string)
        .collect()
}

#[test]
fn dispatch_actions_are_pinned_and_documented_in_dispatch_order() {
    let source = service_source();
    let dispatch = [
        (
            "memory",
            function_section(&source, "memory", "// cas_task -"),
            MEMORY_ACTIONS,
            "skills/cas-memory-management/SKILL.md",
        ),
        (
            "task",
            function_section(&source, "task", "// cas_rule -"),
            TASK_ACTIONS,
            "skills/cas-task-tracking.md",
        ),
        (
            "search",
            function_section(&source, "search", "// cas_system -"),
            SEARCH_ACTIONS,
            "skills/cas-search.md",
        ),
    ];

    for (tool, function, expected, relative) in dispatch {
        let actual = dispatch_actions(function);
        assert_eq!(actual, expected, "{tool} dispatch order changed");

        for flavor in FLAVORS {
            let content = builtin_catalog::find(flavor.catalog, relative);
            assert_eq!(
                documented_actions(&content),
                expected,
                "{} {tool} skill action list drifted",
                flavor.name
            );
        }
    }
}

#[test]
fn memory_request_fields_are_documented_in_source_order() {
    let source = memory_request_source();
    assert_eq!(memory_request_fields(&source), MEMORY_FIELDS);

    for flavor in FLAVORS {
        let content =
            builtin_catalog::find(flavor.catalog, "skills/cas-memory-management/SKILL.md");
        assert_eq!(
            documented_memory_fields(&content),
            MEMORY_FIELDS,
            "{} memory request fields drifted",
            flavor.name
        );
    }
}

#[test]
fn memory_guidance_uses_content_frontmatter_and_live_names() {
    for flavor in FLAVORS {
        let skill = builtin_catalog::find(flavor.catalog, "skills/cas-memory-management/SKILL.md");
        let normalized_skill = skill.to_ascii_lowercase();
        assert!(
            normalized_skill.contains("frontmatter is embedded in the")
                && normalized_skill.contains("`content`")
                && normalized_skill.contains("sqlite-backed entry store"),
            "{} memory skill omits live storage/content guidance",
            flavor.name
        );

        for relative in [
            "skills/cas-memory-management/SKILL.md",
            "skills/cas-memory-management/references/schema.yaml",
            "skills/cas-memory-management/references/body-templates.md",
            "skills/cas-memory-management/references/overlap-detection.md",
            "skills/cas-memory-management/references/lifecycle-and-storage.md",
            "skills/cas-memory-management/references/response-shapes.md",
        ] {
            let content = builtin_catalog::find(flavor.catalog, relative);
            for stale in [
                "cas memory refresh",
                "cas memory migrate",
                "--no-overlap-check",
                "~/.claude/projects/",
                "MEMORY.md index",
            ] {
                assert!(
                    !content.contains(stale),
                    "{} {relative} retains stale memory guidance {stale:?}",
                    flavor.name
                );
            }
        }
    }
}
