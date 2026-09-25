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
        source.contains("\"get\" => \"show\""),
        "task get alias must remain canonicalized to show"
    );
    assert!(
        source.contains("show (also accepted as get)"),
        "task description must document the get alias"
    );
    assert!(
        source.contains("\"inbox\" => \"inbox_poll\""),
        "coordination inbox alias must remain canonicalized to inbox_poll"
    );
    assert!(
        source.contains("inbox_poll (also accepted as inbox)"),
        "coordination description must document the inbox alias"
    );
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
// Call-shape lint (cas-dc1b; audit SYNTHESIS §4, theme T5)
//
// Every suggested `<prefix><tool> action=<a> key=value …` call in the shipped
// builtins (all three flavors) and in the runtime template strings of the Rust
// sources is checked against the live MCP surface:
//   * the action must be a dispatch arm of that tool (aliases included);
//   * every key must be a field of the tool's request struct;
//   * a call that names at least one parameter must carry the fields its
//     handler rejects the call without (see `REQUIRED_FIELDS`).
// A bare name reference such as "use coordination action=message" names no
// parameter and is only checked for the action.
//
// Runtime templates must be clean. Skill-text offenders that Wave B owns are
// allowlisted with their audit master ID; a fixed offender must leave the
// allowlist (stale entries fail) so the list only shrinks.
// ============================================================================

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tool name → request struct. `mcp_search` / `mcp_execute` take no action.
const CALL_SHAPE_TOOLS: &[(&str, &str)] = &[
    ("memory", "MemoryRequest"),
    ("task", "TaskRequest"),
    ("rule", "RuleRequest"),
    ("skill", "SkillRequest"),
    ("coordination", "CoordinationRequest"),
    ("search", "SearchContextRequest"),
    ("system", "SystemRequest"),
    ("verification", "VerificationRequest"),
    ("artifact", "ArtifactRequest"),
    ("knowledge", "KnowledgeRequest"),
    ("team", "TeamRequest"),
    ("pattern", "PatternRequest"),
    ("spec", "SpecRequest"),
];

/// Fields a handler rejects the call without. Kept to rejections that hold
/// for every caller.
const REQUIRED_FIELDS: &[(&str, &str, &[&str])] = &[
    // agent_search_system/message.rs: target, message and summary are required.
    ("coordination", "message", &["target", "summary", "message"]),
    ("coordination", "interrupt", &["target", "summary", "message"]),
    // factory_remind.rs: "remind_message is required for remind action".
    ("coordination", "remind", &["remind_message"]),
    // verification_tools.rs: "Verification requires dispatch_id naming an
    // exact active proof boundary." The capability-bound task-verifier is the
    // one caller whose dispatch comes from its server-side capability; see
    // `DISPATCH_ID_FROM_CAPABILITY`.
    ("verification", "add", &["dispatch_id"]),
    // task/create: `title` always (service/core.rs); `risk` is checked
    // separately because it depends on task_type.
    ("task", "create", &["title"]),
];

/// Builtins whose `verification action=add` runs under a server-bound
/// verifier capability that supplies the dispatch id.
const DISPATCH_ID_FROM_CAPABILITY: &[&str] = &["agents/task-verifier.md"];

/// `task action=create` needs `risk` unless the type is exempt
/// (task/types/task.rs: required for task, bug and feature; task is the
/// default type).
const RISK_EXEMPT_TASK_TYPES: &[&str] = &["epic", "spike", "chore", "gate"];

/// Format-string variables that stand for a prefixed tool in runtime text.
const TEMPLATE_TOOL_VARIABLES: &[(&str, &str)] = &[
    ("coord", "coordination"),
    ("sup_ver", "verification"),
    ("caller_task", "task"),
];

/// Known skill-text offenders, owned by Wave B. Key: builtin-relative path
/// (applies to every flavor that ships it), defect, audit master ID.
const CALL_SHAPE_ALLOWLIST: &[(&str, &str, &str)] = &[
    ("skills/cas-task-tracking/SKILL.md", "task action=create: missing risk", "M12"),
    ("skills/cas-brainstorm/references/handoff.md", "task action=create: missing risk", "M12"),
    ("skills/cas-github-issues/SKILL.md", "task action=create: missing risk", "M12"),
    (
        "skills/cas-ideate/references/post-ideation-workflow.md",
        "task action=create: missing risk",
        "M12",
    ),
    ("skills/cas-supervisor/references/reference.md", "task action=create: missing risk", "M12"),
    ("skills/cas-supervisor/references/workflow.md", "task action=create: missing risk", "M12"),
    (
        "skills/cas-worker/references/details.md",
        "coordination action=message: missing summary",
        "M02",
    ),
    (
        "skills/cas-supervisor/references/worker-recovery.md",
        "coordination action=message: missing summary",
        "M02",
    ),
    (
        "agents/task-verifier.md",
        "verification action=add: unknown field files_reviewed",
        "M10",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CallShapeOffender {
    origin: String,
    line: usize,
    defect: String,
    call: String,
}

struct CallSurface {
    actions: BTreeMap<&'static str, BTreeSet<String>>,
    fields: BTreeMap<&'static str, BTreeSet<String>>,
}

fn request_sources() -> [&'static str; 2] {
    [
        include_str!("../../crates/cas-mcp/src/types.rs"),
        include_str!("../../crates/cas-mcp/src/types/ops_secondary.rs"),
    ]
}

/// Dispatch actions of one tool: the string arms of every
/// `match action.as_str()` / `match req.action.as_str()` in its dispatch
/// function, at that match's own nesting depth (so a nested
/// `match wt_action` cannot contribute its un-prefixed names), plus the
/// aliases canonicalized before dispatch.
fn tool_dispatch_actions(source: &str, tool: &str) -> BTreeSet<String> {
    let start = source
        .find(&format!("pub async fn {tool}("))
        .unwrap_or_else(|| panic!("missing {tool} dispatch function"));
    let body = &source[start + 1..];
    let body = match body.find("pub async fn ") {
        Some(end) => &body[..end],
        None => body,
    };
    let arm = regex::Regex::new(r#"^\|?\s*"[a-z_]+"(\s*\|\s*"[a-z_]+")*\s*\|?$"#).unwrap();
    let header = regex::Regex::new(r"match (\S+) \{\s*$").unwrap();
    let quoted = regex::Regex::new(r#""([a-z_]+)""#).unwrap();

    let mut actions = BTreeSet::new();
    let mut depth: i64 = 0;
    let mut stack: Vec<(i64, bool)> = Vec::new();
    for line in body.lines() {
        while stack.last().is_some_and(|(open, _)| depth < *open) {
            stack.pop();
        }
        if let Some((open, on_action)) = stack.last()
            && depth == *open
            && *on_action
        {
            let left = line.split("=>").next().unwrap_or_default().trim();
            if !left.is_empty() && arm.is_match(left) {
                actions.extend(quoted.captures_iter(left).map(|c| c[1].to_string()));
            }
        }
        depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        if let Some(captures) = header.captures(line) {
            let on_action = matches!(&captures[1], "action.as_str()" | "req.action.as_str()");
            stack.push((depth, on_action));
        }
    }
    match tool {
        "task" => {
            assert!(source.contains("\"get\" => \"show\""));
            actions.insert("get".to_string());
        }
        "coordination" => {
            assert!(source.contains("\"inbox\" => \"inbox_poll\""));
            actions.insert("inbox".to_string());
        }
        _ => {}
    }
    assert!(!actions.is_empty(), "no dispatch actions parsed for {tool}");
    actions
}

fn request_struct_fields(name: &str) -> BTreeSet<String> {
    let marker = format!("pub struct {name} {{");
    let source = request_sources()
        .into_iter()
        .find(|source| source.contains(&marker))
        .unwrap_or_else(|| panic!("missing request struct {name}"));
    let body = source
        .split_once(&marker)
        .unwrap()
        .1
        .split_once("\n}")
        .expect("request struct closing brace")
        .0;
    let fields: BTreeSet<String> = body
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub "))
        .filter_map(|field| field.split_once(':').map(|(name, _)| name.trim().to_string()))
        .collect();
    assert!(fields.contains("action"), "{name} has no action field");
    fields
}

fn call_surface() -> CallSurface {
    let service = service_source();
    let mut actions = BTreeMap::new();
    let mut fields = BTreeMap::new();
    for (tool, request) in CALL_SHAPE_TOOLS {
        actions.insert(*tool, tool_dispatch_actions(service, tool));
        fields.insert(*tool, request_struct_fields(request));
    }
    CallSurface { actions, fields }
}

/// Join `\`-newline continuations (Rust string literals and shell/markdown
/// command blocks) and unescape `\"`, keeping the original line of every
/// output byte.
fn normalize_call_text(text: &str) -> (String, Vec<usize>) {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut lines = Vec::with_capacity(text.len());
    let mut line = 1usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'\n') {
            i += 2;
            line += 1;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                i += 1;
            }
            out.push(' ');
            lines.push(line);
            continue;
        }
        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'"') {
            out.push('"');
            lines.push(line);
            i += 2;
            continue;
        }
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        lines.extend(std::iter::repeat_n(line, ch.len_utf8()));
        if ch == '\n' {
            line += 1;
        }
        i += ch.len_utf8();
    }
    (out, lines)
}

/// Parameter keys that follow a call, stopping at the first token that is
/// not `key=value`. Quoted values may contain spaces.
fn call_parameters(text: &str, mut i: usize) -> (Vec<String>, usize) {
    let bytes = text.as_bytes();
    let key = regex::Regex::new(r"^\[?([a-z_]+)=").unwrap();
    let mut keys = Vec::new();
    loop {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        let Some(captures) = key.captures(&text[i..]) else {
            break;
        };
        keys.push(captures[1].to_string());
        i += captures.get(0).unwrap().end();
        if bytes.get(i) == Some(&b'"') {
            i = text[i + 1..]
                .find('"')
                .map(|end| i + 1 + end + 1)
                .unwrap_or(bytes.len());
        } else {
            while i < bytes.len() && !matches!(bytes[i], b' ' | b'\t' | b'`' | b'\n') {
                i += 1;
            }
        }
    }
    (keys, i)
}

fn lint_call_shapes(
    text: &str,
    origin: &str,
    surface: &CallSurface,
    offenders: &mut Vec<CallShapeOffender>,
) {
    let tools = CALL_SHAPE_TOOLS
        .iter()
        .map(|(tool, _)| *tool)
        .collect::<Vec<_>>()
        .join("|");
    let call = regex::Regex::new(&format!(
        r"(mcp__cas__|mcp__cs__|cas__|cas_|\{{[a-z_]*\}})?({tools}|\{{[a-z_]+\}}) action=([A-Za-z_<>{{}}\-]+)"
    ))
    .unwrap();
    let task_type = regex::Regex::new(r"task_type=([a-z]+)").unwrap();
    let (text, lines) = normalize_call_text(text);

    for captures in call.captures_iter(&text) {
        let whole = captures.get(0).unwrap();
        let prefix = captures.get(1).map(|m| m.as_str());
        let mut tool = captures[2].to_string();
        if let Some(variable) = tool.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
            match TEMPLATE_TOOL_VARIABLES
                .iter()
                .find(|(name, _)| *name == variable)
            {
                Some((_, mapped)) => tool = mapped.to_string(),
                None => continue,
            }
        }
        // An unprefixed tool name must start a word (`subtask action=` is not
        // a `task` call).
        if prefix.is_none()
            && text[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let action = &captures[3];
        if !action.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue; // placeholder such as action=<a>
        }
        let tool = tool.as_str();
        let (keys, end) = call_parameters(&text, whole.end());
        let shown: String = text[whole.start()..end.max(whole.end())]
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        let mut defects = Vec::new();
        if !surface.actions[tool].contains(action) {
            defects.push("unknown action".to_string());
        } else {
            for key in &keys {
                if !surface.fields[tool].contains(key) {
                    defects.push(format!("unknown field {key}"));
                }
            }
            if !keys.is_empty() {
                for (required_tool, required_action, required) in REQUIRED_FIELDS {
                    if *required_tool != tool || *required_action != action {
                        continue;
                    }
                    for field in *required {
                        if *field == "dispatch_id"
                            && DISPATCH_ID_FROM_CAPABILITY
                                .iter()
                                .any(|path| origin.ends_with(path))
                        {
                            continue;
                        }
                        if !keys.iter().any(|key| key == field) {
                            defects.push(format!("missing {field}"));
                        }
                    }
                }
                if tool == "task" && action == "create" && !keys.iter().any(|k| k == "risk") {
                    let declared_type = task_type
                        .captures(&shown)
                        .map(|c| c[1].to_string())
                        .unwrap_or_default();
                    if !RISK_EXEMPT_TASK_TYPES.contains(&declared_type.as_str()) {
                        defects.push("missing risk".to_string());
                    }
                }
            }
        }
        for defect in defects {
            offenders.push(CallShapeOffender {
                origin: origin.to_string(),
                line: lines[whole.start()],
                defect: format!("{tool} action={action}: {defect}"),
                call: shown.clone(),
            });
        }
    }
}

/// Drop `#[cfg(test)] mod … { … }` blocks and `//` comment lines so only
/// runtime text is linted.
fn runtime_text(source: &str) -> String {
    let test_module = regex::Regex::new(r"^\s*(pub(\(crate\))? )?mod \w+ \{").unwrap();
    let lines: Vec<&str> = source.lines().collect();
    let mut kept = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]"
            && lines.get(i + 1).is_some_and(|next| test_module.is_match(next))
        {
            let mut depth: i64 = 0;
            let mut j = i + 1;
            while j < lines.len() {
                depth += lines[j].matches('{').count() as i64
                    - lines[j].matches('}').count() as i64;
                if depth <= 0 && j > i + 1 {
                    break;
                }
                j += 1;
            }
            // Keep line numbers aligned with the source.
            kept.extend(std::iter::repeat_n("", (j + 1).min(lines.len()) - i));
            i = j + 1;
            continue;
        }
        kept.push(if lines[i].trim_start().starts_with("//") {
            ""
        } else {
            lines[i]
        });
        i += 1;
    }
    kept.join("\n")
}

/// Rust sources that carry runtime text: `cas-cli/src` (builtins excluded;
/// they are linted from the compiled catalogs) and `crates/*/src`, skipping
/// every test file and test directory.
fn runtime_rust_sources() -> Vec<(String, PathBuf)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent().expect("workspace root");
    let mut sources = Vec::new();
    for root in [manifest.join("src"), workspace.join("crates")] {
        for entry in walkdir::WalkDir::new(&root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let relative = path
                .strip_prefix(workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let is_test_path = relative.split('/').any(|component| {
                component == "tests"
                    || component.starts_with("test")
                    || component.contains("_test")
            });
            if !entry.file_type().is_file()
                || path.extension().and_then(|e| e.to_str()) != Some("rs")
                || is_test_path
                || relative.contains("/target/")
                || relative.starts_with("cas-cli/src/builtins/")
                || (relative.starts_with("crates/") && !relative.contains("/src/"))
            {
                continue;
            }
            sources.push((relative, path.to_path_buf()));
        }
    }
    assert!(
        sources.iter().any(|(r, _)| r == "crates/cas-pty/src/pty.rs")
            && sources
                .iter()
                .any(|(r, _)| r == "cas-cli/src/ui/factory/director/prompts.rs"),
        "runtime source walk missed the worker contracts"
    );
    sources
}

fn builtin_call_shape_offenders(surface: &CallSurface) -> Vec<CallShapeOffender> {
    let mut offenders = Vec::new();
    for (flavor, label) in builtin_catalog::FLAVORS {
        for builtin in builtin_catalog::skills(*flavor)
            .iter()
            .chain(builtin_catalog::agents(*flavor))
        {
            lint_call_shapes(
                builtin.content,
                &format!("{label}:{}", builtin.path),
                surface,
                &mut offenders,
            );
        }
    }
    offenders
}

fn render_offenders(offenders: &[CallShapeOffender]) -> String {
    offenders
        .iter()
        .map(|o| format!("  {}:{} {} — `{}`", o.origin, o.line, o.defect, o.call))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn call_shape_surface_parses_the_live_dispatch() {
    let surface = call_surface();
    for (tool, action) in [
        ("coordination", "message"),
        ("coordination", "worktree_merge"),
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
    for bogus in ["create", "merge", "status", "worktree", "agent", "factory"] {
        assert!(
            !surface.actions["coordination"].contains(bogus),
            "coordination action set leaked {bogus}"
        );
    }
    assert!(!surface.actions["skill"].contains("get"));
    assert!(!surface.actions["system"].contains("status"));
    assert!(surface.fields["coordination"].contains("summary"));
    assert!(surface.fields["task"].contains("risk"));
    assert!(!surface.fields["verification"].contains("files_reviewed"));
}

/// The lint itself catches the defect classes it exists for.
#[test]
fn call_shape_lint_flags_known_bad_shapes() {
    let surface = call_surface();
    let cases = [
        (
            "`mcp__cas__coordination action=message target=supervisor message=\"x\"`",
            "coordination action=message: missing summary",
        ),
        (
            "use: `{prefix}coordination action=message target={respond_to} message=\\\"...\\\"`",
            "coordination action=message: missing summary",
        ),
        (
            "`mcp__cs__coordination action=remind remind_delay_secs=60`",
            "coordination action=remind: missing remind_message",
        ),
        (
            "{sup_ver} action=add task_id={} status=approved summary=\\\"...\\\"",
            "verification action=add: missing dispatch_id",
        ),
        (
            "`cas__verification action=add task_id=x dispatch_id=d files_reviewed=3 status=approved`",
            "verification action=add: unknown field files_reviewed",
        ),
        (
            "`mcp__cas__task action=create title=\"x\" \\\n  priority=2`",
            "task action=create: missing risk",
        ),
        ("`system action=status`", "system action=status: unknown action"),
    ];
    for (text, expected) in cases {
        let mut offenders = Vec::new();
        lint_call_shapes(text, "fixture", &surface, &mut offenders);
        assert!(
            offenders.iter().any(|o| o.defect == expected),
            "{text:?} must be flagged {expected:?}; got:\n{}",
            render_offenders(&offenders)
        );
    }

    for clean in [
        "`mcp__cas__coordination action=message target=supervisor summary=\"s\" message=\"m\"`",
        "`mcp__cas__task action=create title=\"e\" task_type=epic`",
        "use coordination action=message after the worker registers",
        "the subtask action=create flow",
        "`{coord} action=worktree_merge id=factory/x task_id=cas-1 cleanup=true`",
    ] {
        let mut offenders = Vec::new();
        lint_call_shapes(clean, "fixture", &surface, &mut offenders);
        assert!(
            offenders.is_empty(),
            "{clean:?} must lint clean:\n{}",
            render_offenders(&offenders)
        );
    }
}

/// No runtime template suggests a call the dispatch or schema rejects.
#[test]
fn runtime_templates_suggest_only_calls_the_mcp_surface_accepts() {
    let surface = call_surface();
    let mut offenders = Vec::new();
    for (relative, path) in runtime_rust_sources() {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        lint_call_shapes(&runtime_text(&source), &relative, &surface, &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "runtime templates suggest calls the MCP surface rejects:\n{}",
        render_offenders(&offenders)
    );
}

/// Builtin skill and agent text: every offender is allowlisted with its audit
/// master ID, and every allowlist entry still matches an offender.
#[test]
fn builtin_call_shapes_are_clean_or_allowlisted_with_master_ids() {
    let surface = call_surface();
    let offenders = builtin_call_shape_offenders(&surface);
    for (_, _, master) in CALL_SHAPE_ALLOWLIST {
        assert!(
            master.len() >= 3 && master.starts_with('M') && master[1..].chars().all(|c| c.is_ascii_digit()),
            "allowlist entries must carry an audit master ID, got {master:?}"
        );
    }
    let relative = |origin: &str| origin.split_once(':').map(|(_, path)| path.to_string()).unwrap();

    let unlisted: Vec<CallShapeOffender> = offenders
        .iter()
        .filter(|o| {
            !CALL_SHAPE_ALLOWLIST
                .iter()
                .any(|(path, defect, _)| relative(&o.origin) == *path && o.defect == *defect)
        })
        .cloned()
        .collect();
    assert!(
        unlisted.is_empty(),
        "builtin text suggests calls the MCP surface rejects (fix the text; allowlist only \
         Wave-B skill text, with its master ID):\n{}",
        render_offenders(&unlisted)
    );

    let stale: Vec<_> = CALL_SHAPE_ALLOWLIST
        .iter()
        .filter(|(path, defect, _)| {
            !offenders
                .iter()
                .any(|o| relative(&o.origin) == *path && o.defect == *defect)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "fixed call-shape offenders must leave CALL_SHAPE_ALLOWLIST: {stale:?}"
    );
}
