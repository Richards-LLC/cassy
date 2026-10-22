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

// pin: Extract the declared MCP action surface from handler/type source to validate every builtin call, including uninvoked actions.
fn service_source() -> &'static str {
    include_str!("../src/mcp/tools/service/mod.rs")
}

// pin: Extract the declared MCP action surface from handler/type source to validate every builtin call, including uninvoked actions.
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
// Call-shape surface (cas-dc1b, cas-043b)
//
// The call-shape lint itself is scripts/check-call-shapes.py, a no-build
// release fast row: every suggested `<tool> action=<a> key=value …` call in the
// shipped builtins and in runtime Rust templates must name an accepted action,
// only published fields, and the fields its handler requires. It reads the
// accepted surface from scripts/mcp-call-surface.json; the test below keeps
// that snapshot equal to the live surface.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet};

/// Multi-action tools and every action text may name for them. Proxy-only
/// actions count: shipped text serves builds with `mcp-proxy` (the default).
fn call_shape_action_lists() -> Vec<(&'static str, Vec<&'static str>)> {
    use cas_mcp::actions as accepted;
    vec![
        ("memory", accepted::MEMORY_ACTIONS.to_vec()),
        ("task", accepted::TASK_ACTIONS.to_vec()),
        ("rule", accepted::RULE_ACTIONS.to_vec()),
        ("skill", accepted::SKILL_ACTIONS.to_vec()),
        ("coordination", accepted::COORDINATION_ACTIONS.to_vec()),
        ("factory", accepted::FACTORY_ACTIONS.to_vec()),
        ("search", accepted::SEARCH_ACTIONS.to_vec()),
        (
            "system",
            [accepted::SYSTEM_ACTIONS, accepted::SYSTEM_PROXY_ACTIONS].concat(),
        ),
        (
            "verification",
            [
                accepted::VERIFICATION_ACTIONS,
                accepted::VERIFICATION_PROXY_ACTIONS,
            ]
            .concat(),
        ),
        ("artifact", accepted::ARTIFACT_ACTIONS.to_vec()),
        ("knowledge", accepted::KNOWLEDGE_ACTIONS.to_vec()),
        ("team", accepted::TEAM_ACTIONS.to_vec()),
        ("pattern", accepted::PATTERN_ACTIONS.to_vec()),
        ("spec", accepted::SPEC_ACTIONS.to_vec()),
    ]
}

struct CallSurface {
    actions: BTreeMap<&'static str, BTreeSet<String>>,
    fields: BTreeMap<&'static str, BTreeSet<String>>,
}

/// Accepted actions from `cas_mcp::actions`; fields from the published
/// `tools/list` input schemas (the same surface the model sees).
fn call_surface() -> CallSurface {
    let published = published_tools();
    let mut actions = BTreeMap::new();
    let mut fields = BTreeMap::new();
    for (tool, accepted) in call_shape_action_lists() {
        actions.insert(tool, accepted.into_iter().map(str::to_string).collect());
        let schema = published
            .iter()
            .find(|candidate| candidate.name == tool)
            .unwrap_or_else(|| panic!("{tool} tool is not registered"));
        let properties: BTreeSet<String> = schema.input_schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool} input schema has no properties"))
            .keys()
            .cloned()
            .collect();
        assert!(properties.contains("action"), "{tool} schema has no action");
        fields.insert(tool, properties);
    }
    CallSurface { actions, fields }
}

/// The call-shape lint's committed surface equals the live one: accepted actions
/// from `cas_mcp::actions` and fields from the published `tools/list` schemas.
#[test]
fn call_shape_surface_snapshot_matches_the_published_surface() {
    let surface = call_surface();
    let tools: serde_json::Map<String, serde_json::Value> = surface
        .actions
        .iter()
        .map(|(tool, actions)| {
            let entry = serde_json::json!({"actions": actions, "fields": surface.fields[tool]});
            (tool.to_string(), entry)
        })
        .collect();
    let live = serde_json::to_string_pretty(&serde_json::json!({ "tools": tools })).unwrap() + "\n";
    let committed = include_str!("../../scripts/mcp-call-surface.json");
    if committed != live {
        let fresh = std::env::temp_dir().join("mcp-call-surface.json");
        std::fs::write(&fresh, &live).unwrap();
        panic!(
            "scripts/mcp-call-surface.json is stale: the MCP surface changed. Regenerated: \
             cp {} scripts/mcp-call-surface.json",
            fresh.display()
        );
    }
}

#[test]
fn call_shape_surface_comes_from_the_published_action_lists() {
    let surface = call_surface();
    for (tool, action) in [
        ("coordination", "message"),
        ("factory", "worktree_merge"),
        ("factory", "spawn_workers"),
        ("coordination", "interrupt"),
        ("coordination", "inbox"),
        ("task", "get"),
        ("task", "request_changes"),
        ("verification", "add"),
        ("skill", "show"),
        ("system", "proxy_list"),
    ] {
        assert!(surface.actions[tool].contains(action), "{tool} lacks {action}");
    }
    // Names of the nested worktree match and domain labels are not actions.
    // `worktree_merge` must also be accepted by the published schema.
    for bogus in ["create", "merge", "status", "worktree", "agent", "factory"] {
        assert!(
            !surface.actions["coordination"].contains(bogus),
            "coordination action set leaked {bogus}"
        );
    }
    // cas-8563b (D2): fleet control is only on `factory`, messaging only on
    // `coordination`.
    for moved in ["spawn_workers", "worktree_merge", "epic_status", "server_start"] {
        assert!(!surface.actions["coordination"].contains(moved), "{moved}");
    }
    for external_work in cas_mcp::actions::EXTERNAL_WORK_ACTIONS {
        assert!(
            !surface.actions["coordination"].contains(*external_work),
            "coordination publishes supervisor-only action {external_work}"
        );
        assert!(
            surface.actions["factory"].contains(*external_work),
            "factory is missing supervisor-only action {external_work}"
        );
    }
    assert!(!surface.actions["factory"].contains("message"));
    assert!(!surface.fields["factory"].contains("summary"));
    assert!(!surface.fields["coordination"].contains("worker_names"));
    assert!(!surface.actions["skill"].contains("get"));
    assert!(!surface.actions["system"].contains("status"));
    assert!(surface.fields["coordination"].contains("summary"));
    for external_work_field in [
        "external_ref",
        "request_id",
        "resource_id",
        "handoff_id",
        "expected_generation",
        "to_slot_id",
        "offer_ttl_secs",
    ] {
        assert!(
            !surface.fields["coordination"].contains(external_work_field),
            "coordination publishes external-work field {external_work_field}"
        );
        assert!(
            surface.fields["factory"].contains(external_work_field),
            "factory is missing external-work field {external_work_field}"
        );
    }
    assert!(surface.fields["task"].contains("risk"));
    assert!(!surface.fields["verification"].contains("files_reviewed"));
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
        .or_else(|| source.find(&format!("async fn {tool}(")))
        .unwrap_or_else(|| panic!("missing {tool} dispatch function"));
    let function = &source[start..];
    // Take whichever dispatch form comes FIRST after this function's start.
    // Preferring one spelling and falling back to the other searched past the
    // end of `memory` (which matches on `req.action`) into `task`'s table.
    let match_start = [
        "let result = match action.as_str() {",
        "let result = match req.action.as_str() {",
    ]
    .iter()
    .filter_map(|needle| function.find(needle))
    .min()
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
    // The handler table coordination() and factory() share.
    let coordination_handlers: BTreeSet<String> =
        top_level_dispatch_literals(&source, "coordination_dispatch")
            .into_iter()
            .collect();
    let aliases_of = |name: &str| -> &'static [(&'static str, &'static str)] {
        match name {
            "task" => TASK_ACTION_ALIASES,
            "coordination" => COORDINATION_ACTION_ALIASES,
            _ => &[],
        }
    };
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

        // cas-2d86: coordination() and factory() route on the registry list
        // they publish (cas-269ab), so their own match holds no literals. What
        // they can dispatch is that list, narrowed to the actions that reach a
        // handler arm in the shared `coordination_dispatch` table.
        let mut dispatched: BTreeSet<String> = match shared_registry_routing(&name) {
            Some(registry) => {
                let routing = tool_function(&source, &name);
                let membership = format!("cas_mcp::actions::{}.contains(", registry.0);
                assert!(
                    routing.contains(&membership),
                    "{name} must route on the registry it publishes ({membership}...)"
                );
                registry
                    .1
                    .iter()
                    .filter(|action| coordination_handlers.contains(**action))
                    .map(|action| (*action).to_string())
                    .filter(|action| {
                        !aliases_of(&name).iter().any(|(alias, _)| alias == action)
                    })
                    .collect()
            }
            None => top_level_dispatch_literals(&source, &name)
                .into_iter()
                .filter(|action| {
                    cfg!(feature = "mcp-proxy") || !proxy_only.contains(&action.as_str())
                })
                .collect(),
        };
        for (alias, canonical) in aliases_of(&name) {
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
        checked, 14,
        "every multi-action tool publishes an action enum"
    );
    // cas-2d86: and the reverse: every handler arm in the shared table is
    // published by coordination or factory, so nothing dispatchable hides.
    let shared: BTreeSet<String> = cas_mcp::actions::COORDINATION_ACTIONS
        .iter()
        .chain(cas_mcp::actions::FACTORY_ACTIONS)
        .map(|action| (*action).to_string())
        .collect();
    let unpublished: Vec<&String> = coordination_handlers.difference(&shared).collect();
    assert!(
        unpublished.is_empty(),
        "coordination_dispatch handles unpublished actions: {unpublished:?}"
    );
}

/// cas-2d86: tools that route on a `cas_mcp::actions` registry list instead
/// of literal arms, with the list they publish.
fn shared_registry_routing(tool: &str) -> Option<(&'static str, &'static [&'static str])> {
    match tool {
        "coordination" => Some(("COORDINATION_ACTIONS", cas_mcp::actions::COORDINATION_ACTIONS)),
        "factory" => Some(("FACTORY_ACTIONS", cas_mcp::actions::FACTORY_ACTIONS)),
        _ => None,
    }
}

/// The source of one `#[tool]` function, up to the next tool attribute.
fn tool_function<'a>(source: &'a str, tool: &str) -> &'a str {
    let start = source
        .find(&format!("pub async fn {tool}("))
        .unwrap_or_else(|| panic!("missing {tool} tool function"));
    let rest = &source[start..];
    let end = rest[1..].find("#[tool(").map_or(rest.len(), |end| end + 1);
    &rest[..end]
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
    if payload.len() > TOOLS_LIST_BUDGET_BYTES {
        // cas-78ee: name what to trim, largest first.
        let descriptions: Vec<(usize, String)> = tools
            .iter()
            .map(|tool| (tool.description.as_ref().map_or(0, |text| text.len()), tool.name.to_string()))
            .collect();
        let mut properties: Vec<(usize, String)> = tools
            .iter()
            .flat_map(|tool| {
                let properties = tool.input_schema["properties"].as_object().cloned().unwrap_or_default();
                properties.into_iter().map(move |(name, schema)| {
                    (serde_json::to_string(&schema).unwrap().len(), format!("{}.{name}", tool.name))
                })
            })
            .collect();
        properties.sort_by(|left, right| right.cmp(left));
        properties.truncate(30);
        panic!(
            "tools/list is {} bytes; budget {TOOLS_LIST_BUDGET_BYTES}. Tool descriptions: {descriptions:?}. \
             Largest properties: {properties:?}",
            payload.len()
        );
    }
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
    // cas-8563b (D2): spawn and sync parameters are published on `factory`.
    let coordination = published_tools()
        .into_iter()
        .find(|tool| tool.name == "factory")
        .expect("factory tool");
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
