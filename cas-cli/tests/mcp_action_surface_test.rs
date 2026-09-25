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
    "get",
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
        .find("let result = match action.as_str() {")
        .or_else(|| section.find("let result = match req.action.as_str() {"))
        .expect("dispatch result match");
    let arms = &section[match_start..];
    let arms = arms
        .split_once('{')
        .expect("dispatch match opening brace")
        .1;

    let mut actions: Vec<String> = arms
        .lines()
        .take_while(|line| !line.trim_start().starts_with("_ =>"))
        .filter_map(|line| line.split_once("=>").map(|(left, _)| left))
        .flat_map(|left| left.split('|'))
        .map(str::trim)
        .filter(|token| token.starts_with('"') && token.ends_with('"'))
        .map(|token| token.trim_matches('"').to_string())
        .collect();

    // Aliases are canonicalized before the dispatch match, so they are not
    // represented by a second match arm. Keep them in the pinned/documented
    // surface immediately beside their canonical action.
    if section.contains("canonical_task_action") {
        let show = actions
            .iter()
            .position(|action| action == "show")
            .expect("task dispatch show action");
        actions.insert(show + 1, "get".to_string());
    }
    actions
}

fn section<'a>(content: &'a str, heading: &str) -> &'a str {
    let start = content
        .find(heading)
        .unwrap_or_else(|| panic!("missing {heading:?}"));
    let body = &content[start + heading.len()..];
    let end = body.find("\n## ").unwrap_or(body.len());
    &body[..end]
}

#[test]
fn canonicalized_aliases_are_pinned_and_described() {
    let source = service_source();
    let task = function_section(&source, "task", "// cas_rule -");
    assert_eq!(
        dispatch_actions(task)
            .into_iter()
            .find(|action| action == "get"),
        Some("get".to_string())
    );
    assert!(
        cas_mcp::actions::TASK_ACTION_ALIASES.contains(&("get", "show")),
        "task get alias must remain canonicalized to show"
    );
    assert!(
        source.contains("canonical_action(cas_mcp::actions::TASK_ACTION_ALIASES, action)"),
        "task dispatch must canonicalize through the published alias table"
    );
    assert!(
        cas_mcp::actions::COORDINATION_ACTION_ALIASES.contains(&("inbox", "inbox_poll")),
        "coordination inbox alias must remain canonicalized to inbox_poll"
    );
    assert!(
        source.contains("canonical_action(cas_mcp::actions::COORDINATION_ACTION_ALIASES, action)"),
        "coordination dispatch must canonicalize through the published alias table"
    );
    for (tool, documented) in [
        ("task", "`get` is an alias of `show`"),
        ("coordination", "`inbox` is an alias of `inbox_poll`"),
    ] {
        let description = published_action(tool)["description"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            description.contains(documented),
            "{tool} action description must document its alias: {description}"
        );
    }
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

// ============================================================================
// Published tool surface (tools/list)
// ============================================================================

/// Claude Code cuts tool descriptions at 2 KB without warning.
const CLAUDE_CODE_DESCRIPTION_CAP: usize = 2_048;

/// `tools/list` for 3.31.0 was 69,635 bytes (compact JSON). The schema diet
/// must keep at least 10 KB of that off.
const TOOLS_LIST_BUDGET_BYTES: usize = 59_000;

fn published_tools() -> Vec<rmcp::model::Tool> {
    cas::mcp::tools::CasService::tool_definitions_for_build()
}

fn published_action(tool: &str) -> serde_json::Value {
    let tool = published_tools()
        .into_iter()
        .find(|candidate| candidate.name == tool)
        .unwrap_or_else(|| panic!("{tool} tool is not registered"));
    tool.input_schema["properties"]["action"].clone()
}

/// Every string literal in pattern position of the tool's top-level dispatch
/// `match`. Nested matches, arm bodies, attributes and comments are skipped,
/// so multi-line `"a" | "b"` arms and `#[cfg]`-gated arms are both read.
fn top_level_dispatch_literals(source: &str, tool: &str) -> Vec<String> {
    let start = source
        .find(&format!("pub async fn {tool}("))
        .unwrap_or_else(|| panic!("missing {tool} dispatch function"));
    let function = &source[start..];
    let match_start = function
        .find("let result = match action.as_str() {")
        .or_else(|| function.find("let result = match req.action.as_str() {"))
        .unwrap_or_else(|| panic!("missing {tool} dispatch match"));
    let body = &function[match_start..];
    let body = &body[body.find('{').expect("match opening brace") + 1..];
    let chars: Vec<char> = body.chars().collect();

    let mut literals = Vec::new();
    let mut depth = 0usize;
    let mut in_pattern = true;
    let mut block_arm = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '"' => {
                let mut literal = String::new();
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '"' {
                    if chars[j] == '\\' {
                        j += 1;
                    }
                    if let Some(&ch) = chars.get(j) {
                        literal.push(ch);
                    }
                    j += 1;
                }
                if depth == 0 && in_pattern {
                    literals.push(literal);
                }
                i = j + 1;
                continue;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') => {
                i += 3;
                continue;
            }
            '=' if next == Some('>') && depth == 0 && in_pattern => {
                in_pattern = false;
                let rest = chars[i + 2..].iter().find(|ch| !ch.is_whitespace());
                block_arm = rest == Some(&'{');
                i += 2;
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                if depth == 0 {
                    break; // end of the dispatch match
                }
                depth -= 1;
                if depth == 0 && c == '}' && block_arm && !in_pattern {
                    in_pattern = true;
                    block_arm = false;
                }
            }
            ',' if depth == 0 => {
                in_pattern = true;
                block_arm = false;
            }
            _ => {}
        }
        i += 1;
    }
    literals
}

#[test]
fn published_action_enums_equal_their_dispatch_tables() {
    use cas_mcp::actions::{COORDINATION_ACTION_ALIASES, TASK_ACTION_ALIASES};
    use std::collections::BTreeSet;

    let source = service_source();
    let proxy_only = [
        "proxy_add",
        "proxy_remove",
        "proxy_list",
        "proxy_health",
        "external_verify",
    ];
    let mut checked = 0;
    for tool in published_tools() {
        let action = &tool.input_schema["properties"]["action"];
        if action.is_null() {
            continue; // mcp_search / mcp_execute take no action
        }
        let name = tool.name.to_string();
        let published: BTreeSet<String> = action["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} action must be an enum: {action}"))
            .iter()
            .map(|value| value.as_str().expect("enum values are strings").to_string())
            .collect();

        let mut dispatched: BTreeSet<String> = top_level_dispatch_literals(&source, &name)
            .into_iter()
            .filter(|action| cfg!(feature = "mcp-proxy") || !proxy_only.contains(&action.as_str()))
            .collect();
        let aliases: &[(&str, &str)] = match name.as_str() {
            "task" => TASK_ACTION_ALIASES,
            "coordination" => COORDINATION_ACTION_ALIASES,
            _ => &[],
        };
        for (alias, canonical) in aliases {
            assert!(
                dispatched.contains(*canonical),
                "{name} alias {alias} points at an undispatched action {canonical}"
            );
            dispatched.insert((*alias).to_string());
        }
        assert!(
            !dispatched.is_empty(),
            "{name} dispatch table was not parsed"
        );
        assert_eq!(
            published, dispatched,
            "{name} action enum drifted from its dispatch table"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 13,
        "every multi-action tool publishes an action enum"
    );
}

#[test]
fn every_tool_description_fits_the_claude_code_cap() {
    for tool in published_tools() {
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            !description.is_empty() && description.chars().count() <= CLAUDE_CODE_DESCRIPTION_CAP,
            "{} description is {} chars; Claude Code truncates at {CLAUDE_CODE_DESCRIPTION_CAP}",
            tool.name,
            description.chars().count()
        );
        if tool.name == "coordination" {
            assert!(
                description.chars().count() <= 1_500,
                "coordination description must stay a purpose line plus action groups"
            );
        }
        assert!(
            !description.contains("IMPORTANT"),
            "{} description shouts; the rule belongs on the parameter it governs",
            tool.name
        );
    }
}

#[test]
fn tools_list_carries_no_schema_boilerplate() {
    let tools = published_tools();
    let payload = serde_json::to_string(&tools).expect("tools serialize");
    for boilerplate in [
        "\"nullable\"",
        "\"default\":null",
        "\"$schema\"",
        "\"format\":\"uint",
        "\"format\":\"int",
        "\"format\":\"float",
        "\"format\":\"double",
    ] {
        assert!(
            !payload.contains(boilerplate),
            "tools/list still carries {boilerplate}"
        );
    }
    for tool in &tools {
        assert!(
            tool.input_schema.get("title").is_none(),
            "{} input schema still has a root title",
            tool.name
        );
    }
    assert!(
        payload.len() <= TOOLS_LIST_BUDGET_BYTES,
        "tools/list is {} bytes; budget {TOOLS_LIST_BUDGET_BYTES}",
        payload.len()
    );
}

#[test]
fn agent_visible_tool_text_has_no_ticket_ids_or_stale_values() {
    let payload = serde_json::to_string(&published_tools()).expect("tools serialize");
    let ticket = regex::Regex::new(r"\(cas-[0-9a-f]{4}\)|cassy#[0-9]+|GH #[0-9]+").unwrap();
    assert!(
        !ticket.is_match(&payload),
        "ticket ids in agent-visible tool text: {:?}",
        ticket.find(&payload).map(|m| m.as_str())
    );
    for stale in [
        "claude-opus-4-5",
        "'claude' (default) or 'codex'",
        "TypeScript code",
        "Claude-only",
    ] {
        assert!(!payload.contains(stale), "stale tool text {stale:?}");
    }
    let coordination = published_tools()
        .into_iter()
        .find(|tool| tool.name == "coordination")
        .expect("coordination tool");
    let config_dir = coordination.input_schema["properties"]["config_dir"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        config_dir.contains("CODEX_HOME"),
        "config_dir must name CODEX_HOME"
    );
    let force = coordination.input_schema["properties"]["force"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        force.contains("live worker's worktree is always skipped"),
        "force must state that live worktrees are never synced: {force}"
    );
}
