//! rule-172: inventory serde spellings, then prove mappings by parsing captures.
//! This reads declaration metadata only; it never inspects handler logic.
use super::HookInput;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};

type Bindings = BTreeMap<String, String>;

// Compatibility spellings already shipped at aa64c907 without captured evidence.
// They are not captures. A new or moved spelling needs an actual capture below.
const LEGACY: &[(&str, &str)] = &[
    ("sessionId", "session_id"),
    ("transcriptPath", "transcript_path"),
    ("workspaceRoot", "workspace_root"),
    ("permissionMode", "permission_mode"),
    ("hookEventName", "hook_event_name"),
    ("toolName", "tool_name"),
    ("toolInput", "tool_input"),
    ("toolResult", "tool_response"),
    ("toolUseId", "tool_use_id"),
    ("toolInputTruncated", "tool_input_truncated"),
    ("userPrompt", "user_prompt"),
    ("machinePromptProvenance", "machine_prompt_provenance"),
    ("subagentType", "subagent_type"),
    ("stopHookActive", "stop_hook_active"),
    ("agentId", "agent_id"),
    ("agentType", "agent_type"),
    ("subagentPrompt", "subagent_prompt"),
];

struct Capture {
    name: &'static str,
    raw: &'static str,
    // Independent observed-wire -> consumed-field contracts, not generated
    // from HookInput. The parsed value must also differ from its default.
    bindings: &'static [(&'static str, &'static str)],
}
const CAPTURES: &[Capture] = &[
    Capture {
        name: "Claude 2.1.265 UserPromptSubmit (fixture paths sanitized)",
        raw: include_str!("fixtures/claude-2.1.265-user-prompt-submit.json"),
        bindings: &[("prompt", "user_prompt")],
    },
    Capture {
        name: "Claude 2.1.224 MessageDisplay (cas-f3e3)",
        raw: include_str!("fixtures/cas-f3e3-message-display.json"),
        bindings: &[("delta", "message"), ("final", "message_is_final")],
    },
];

/// Extract only HookInput field-level serde metadata. Fail closed if declaration
/// syntax changes rather than quietly ignoring an unrecognised attribute.
fn declarations(source: &str) -> (Bindings, Bindings) {
    let body = source
        .split_once("pub struct HookInput {")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    assert!(
        !body.contains("/*"),
        "unsupported block comment in HookInput metadata; update inventory parser"
    );
    let body = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let fields = Regex::new(r"(?s)#\[serde\((.*?)\)\]\s*pub\s+(\w+)\s*:").unwrap();
    let spellings = Regex::new(r#"\b(alias|rename)\s*=\s*("(?:\\.|[^"\\])*")"#).unwrap();
    let keywords = Regex::new(r"\b(?:alias|rename)\s*=").unwrap();
    let mut aliases = BTreeMap::new();
    let mut serialized = BTreeMap::new();
    for field in fields.captures_iter(&body) {
        let attributes = &field[1];
        assert!(
            !attributes.contains("pub ") && !attributes.contains("#["),
            "unsupported HookInput attribute syntax; update metadata inventory"
        );
        let name = field[2].to_string();
        let mut serialization_name = name.clone();
        let entries: Vec<_> = spellings.captures_iter(attributes).collect();
        assert_eq!(
            entries.len(),
            keywords.find_iter(attributes).count(),
            "unsupported serde spelling literal"
        );
        let remainder = spellings.replace_all(attributes, "");
        assert!(
            remainder
                .split(',')
                .all(|part| matches!(part.trim(), "" | "default")),
            "unsupported HookInput serde option; extend the inventory parser and wire contract"
        );
        for entry in entries {
            let wire: String = serde_json::from_str(&entry[2]).unwrap();
            if &entry[1] == "rename" {
                serialization_name = wire.clone();
            }
            assert!(
                aliases.insert(wire, name.clone()).is_none(),
                "duplicate serde wire spelling"
            );
        }
        serialized.insert(name, serialization_name);
    }
    assert_eq!(
        serialized.len(),
        body.matches("#[serde(").count(),
        "a HookInput field escaped the serde metadata inventory"
    );
    (aliases, serialized)
}

#[test]
fn every_hook_alias_or_rename_has_a_captured_parse_contract_or_legacy_binding() {
    let (declared, serialized) = declarations(include_str!("types.rs"));
    let mut covered: Bindings = LEGACY
        .iter()
        .map(|(wire, field)| (wire.to_string(), field.to_string()))
        .collect();
    let defaults = serde_json::to_value(HookInput::default()).unwrap();
    for capture in CAPTURES {
        // Both consumers receive the same untouched captured JSON bytes.
        let wire: serde_json::Value = serde_json::from_str(capture.raw).unwrap();
        let parsed: HookInput = serde_json::from_str(capture.raw).unwrap();
        let observed = serde_json::to_value(parsed).unwrap();
        for &(key, field) in capture.bindings {
            assert!(
                wire.get(key).is_some_and(|v| !v.is_null()),
                "{}: capture lacks meaningful {key}",
                capture.name
            );
            let serialized_key = serialized
                .get(field)
                .expect("contract names an existing field");
            assert_eq!(
                observed.get(serialized_key),
                wire.get(key),
                "{}: wire {key} did not reach {field}",
                capture.name
            );
            assert_ne!(
                observed.get(serialized_key),
                defaults.get(serialized_key),
                "{}: captured {key} cannot distinguish a silent drop from the default; retain a discriminating capture",
                capture.name
            );
            assert!(
                covered.insert(key.into(), field.into()).is_none(),
                "contract overlaps legacy/another capture binding"
            );
        }
    }
    assert_eq!(
        declared, covered,
        "changed hook spellings require a captured raw payload + field parse contract; remove stale inventory entries too"
    );
}

#[test]
fn inventory_catches_new_moved_and_deleted_serde_bindings_without_handler_pins() {
    let source = include_str!("types.rs");
    let (baseline, _) = declarations(source);
    let mut expected: BTreeSet<_> = LEGACY
        .iter()
        .map(|(wire, field)| (wire.to_string(), field.to_string()))
        .collect();
    for capture in CAPTURES {
        expected.extend(
            capture
                .bindings
                .iter()
                .map(|(wire, field)| (wire.to_string(), field.to_string())),
        );
    }
    assert_eq!(
        baseline.clone().into_iter().collect::<BTreeSet<_>>(),
        expected
    );
    for (case, mutated) in [
        (
            "new alias alongside an existing captured alias",
            source.replacen(
                "alias = \"prompt\"",
                "alias = \"prompt\", alias = \"newHarnessPrompt\"",
                1,
            ),
        ),
        (
            "new rename on a previously default-named field",
            source.replacen(
                "#[serde(default)]\n    pub cwd:",
                "#[serde(default, rename = \"workingDirectory\")]\n    pub cwd:",
                1,
            ),
        ),
        (
            "changed prompt alias",
            source.replacen("alias = \"prompt\"", "alias = \"newHarnessPrompt\"", 1),
        ),
        (
            "changed delta alias",
            source.replacen("alias = \"delta\"", "alias = \"wrongDelta\"", 1),
        ),
        (
            "changed final rename",
            source.replacen("rename = \"final\"", "rename = \"isFinal\"", 1),
        ),
        (
            "removed delta alias",
            source.replacen(", alias = \"delta\"", "", 1),
        ),
        (
            "prompt alias moved to the unconsumed subagent field (cas-78d3)",
            source.replacen(", alias = \"prompt\"", "", 1).replacen(
                "alias = \"subagentPrompt\"",
                "alias = \"subagentPrompt\", alias = \"prompt\"",
                1,
            ),
        ),
        (
            "changed consumed field",
            source.replacen(
                "pub user_prompt: Option<String>",
                "pub wrong_prompt: Option<String>",
                1,
            ),
        ),
    ] {
        assert_ne!(mutated, source, "mutation did not apply: {case}");
        assert_ne!(
            declarations(&mutated).0,
            baseline,
            "wire contract inventory missed {case}"
        );
    }
}
