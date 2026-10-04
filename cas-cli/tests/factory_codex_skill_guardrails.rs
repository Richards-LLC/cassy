use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/builtin_catalog.rs"]
mod builtin_catalog;

fn load(path: &Path) -> &'static str {
    let path = path.to_string_lossy();
    let relative = path
        .split_once("cas-cli/src/builtins/")
        .map(|(_, relative)| format!("cas-cli/src/builtins/{relative}"))
        .unwrap_or_else(|| panic!("not an embedded builtin source path: {path}"));
    builtin_catalog::find_source_path(&relative)
}

/// Build source-shaped paths for the static loader without resolving them
/// against the checkout. The path is only a catalog lookup key.
fn source_root() -> PathBuf {
    PathBuf::new()
}

#[test]
fn codex_factory_skills_use_cs_prefix_only() {
    let root = cas::test_paths::workspace_root();
    let skills_dir = root.join(".codex/skills");
    if !skills_dir.exists() {
        eprintln!(
            "SKIP Codex projection check: source checkout has no .codex/skills at {}",
            skills_dir.display()
        );
        return;
    }

    let entries = fs::read_dir(&skills_dir).expect("read .codex/skills");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("cas-factory-") {
            continue;
        }
        let skill_path = entry.path().join("SKILL.md");
        if !skill_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&skill_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", skill_path.display()));
        let has_mcp_examples = content.contains("mcp__cs__") || content.contains("mcp__cas__");
        if has_mcp_examples {
            assert!(
                content.contains("mcp__cs__"),
                "{} should include mcp__cs__ examples",
                skill_path.display()
            );
        }
        assert!(
            !content.contains("mcp__cas__"),
            "{} still contains legacy mcp__cas__ references",
            skill_path.display()
        );
        assert!(
            !content.contains("action=prompt"),
            "{} still contains legacy action=prompt usage",
            skill_path.display()
        );
    }
}

#[test]
fn codex_worker_recovery_uses_cs_alias_not_cas(/* cas-5b4f */) {
    // cas-5b4f, audit D1: a Codex worker cannot call `mcp__cas__` tools. The
    // one recovery guide every harness installs names tools by bare name and
    // spells no prefix literal; the role guidance states each prefix once.
    let root = source_root();
    for flavor in ["", "codex/", "grok/"] {
        let recovery = load(&root.join(format!(
            "cas-cli/src/builtins/{flavor}skills/cas-worker/references/recovery.md"
        )));
        let offenders = cas::builtins::unsanctioned_prefixed_tool_lines(recovery);
        assert!(
            offenders.is_empty(),
            "{flavor} worker recovery.md spells a harness prefix outside the naming rule: {offenders:?}"
        );
    }
}

#[test]
fn codex_builtin_supervisor_guide_includes_core_workflow() {
    let root = source_root();
    let guide = root.join("cas-cli/src/builtins/codex/skills/cas-supervisor.md");
    let content = load(&guide);
    assert!(
        content.contains(cas::builtins::TOOL_NAMING_LINE),
        "codex supervisor guide should state the per-harness tool prefix"
    );
}

/// cas-314d: supervisors and workers share one bounded, push-first reminder
/// contract. The on-demand reference avoids inflating the SessionStart skill
/// bodies, but all three installed flavors must retain identical semantics.
#[test]
fn reminder_discipline_reference_is_complete_and_flavor_normalized() {
    let root = source_root();
    let claude =
        load(&root.join("cas-cli/src/builtins/skills/cas-supervisor/references/reminders.md"));
    let codex = load(
        &root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/reminders.md"),
    );
    let grok =
        load(&root.join("cas-cli/src/builtins/grok/skills/cas-supervisor/references/reminders.md"));

    assert_eq!(claude, codex);
    assert_eq!(claude, grok);
}

#[test]
fn supervisor_epic_driving_reference_is_compact_and_three_way_mirrored() {
    let root = source_root();
    let paths = [
        root.join("cas-cli/src/builtins/skills/cas-supervisor/references/epic-driving.md"),
        root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/epic-driving.md"),
        root.join("cas-cli/src/builtins/grok/skills/cas-supervisor/references/epic-driving.md"),
    ];
    let contents: Vec<&'static str> = paths.iter().map(|path| load(path)).collect();

    for (path, content) in paths.iter().zip(&contents) {
        assert!(
            content.len() < 2 * 1024,
            "{} exceeds the 2KB operator budget ({} bytes)",
            path.display(),
            content.len()
        );
    }

    assert_eq!(contents[0], contents[1]);
    assert_eq!(contents[0], contents[2]);
}

#[test]
fn supervisor_skill_mirrors_include_implementation_unit_template() {
    // After cas-61af split cas-supervisor.md into a main file + references,
    // the Implementation Unit Template moved to planning.md. The guardrail
    // checks that file instead (both .claude and .codex trees must match).
    let root = source_root();
    let claude =
        load(&root.join("cas-cli/src/builtins/skills/cas-supervisor/references/planning.md"));
    let codex =
        load(&root.join("cas-cli/src/builtins/codex/skills/cas-supervisor/references/planning.md"));

    for (label, content) in [("claude", &claude), ("codex", &codex)] {
        // Canonical template markers (R1)

        // R4 mapping table

        // R13 cross-link: Spec Requirements section mentions the template
        let spec_idx = content
            .find("## Spec Requirements")
            .unwrap_or_else(|| panic!("{label} missing Spec Requirements heading"));
        let tmpl_idx = content
            .find("## Implementation Unit Template")
            .unwrap_or_else(|| panic!("{label} missing Implementation Unit Template heading"));
        let spec_block = &content[spec_idx..tmpl_idx];
        assert!(
            spec_block.contains("Implementation Unit Template"),
            "{label} Spec Requirements section missing cross-link to Implementation Unit Template"
        );
    }
}

/// cas-2c61/cas-62ab, audit D1: no Codex catalog entry hardcodes Claude's
/// `mcp__cas__` alias as an instruction. Tool names are bare; a prefix is
/// spelled only in the naming line and generated agent `tools:` frontmatter.
#[test]
fn codex_builtin_skills_and_agents_never_hardcode_claude_alias() {
    for builtin in builtin_catalog::skills(builtin_catalog::Flavor::Codex)
        .iter()
        .chain(builtin_catalog::agents(builtin_catalog::Flavor::Codex))
    {
        if !matches!(builtin.path.rsplit('.').next(), Some("md" | "yaml")) {
            continue;
        }
        let offenders = cas::builtins::unsanctioned_prefixed_tool_lines(builtin.content);
        assert!(
            offenders.is_empty(),
            "codex {} spells a harness prefix outside the naming rule: {offenders:?}",
            builtin.path
        );
    }
}

#[test]
fn codex_worker_runtime_instruction_allows_close_then_escalate() {
    // cas-8563b: the Codex worker contract is rendered by the shared
    // `worker_contract` renderer with the `mcp__cs__` prefix, so check the
    // rendered launch surface rather than a literal in the source file.
    let content = cas_mux::rendered_contract_surface("codex", cas_mux::ContractRole::Worker);

    // The rendered call shape is the contract; surrounding prose may change.
    assert!(
        content.contains("`mcp__cs__task action=close"),
        "runtime worker instruction should instruct workers to close tasks"
    );
}

/// GH #1111: a Codex QA reviewer must discover verification and record its own
/// verdict, even though ordinary implementation verification is supervisor-owned.
#[test]
fn cas_bb7e_qa_reviewers_load_verification_in_worker_contract() {
    for (harness, prefix) in [
        ("codex", "mcp__cs__"),
        ("claude", "mcp__cas__"),
        ("grok", "cas__"),
    ] {
        let contract = cas_mux::rendered_contract_surface(harness, cas_mux::ContractRole::Worker);
        assert!(
            contract.contains(&format!("`{prefix}verification`")),
            "{contract}"
        );
        assert!(
            contract.contains(&format!("{prefix}verification action=qa_record")),
            "{contract}"
        );
        assert!(contract.contains("QA-pass task"), "{contract}");
        assert!(
            !contract.contains(&format!(
                "Load only `{prefix}task` and `{prefix}coordination`;"
            )),
            "{contract}"
        );
        assert!(
            contract.contains("ask the supervisor to verify and close"),
            "{contract}"
        );
    }
}

#[test]
fn cas_bb7e_qa_record_is_in_the_published_codex_mcp_catalog() {
    // Codex injects `cas serve` as `cs`; this is the same compact catalog
    // tools/list returns, rather than an inventory maintained by this test.
    let tools = cas::mcp::tools::service::CasService::tool_definitions_for_build();
    let verification = tools
        .iter()
        .find(|tool| tool.name == "verification")
        .expect("Codex's mcp__cs__verification must be discoverable");
    let properties = verification.input_schema.get("properties").unwrap();
    assert!(
        properties["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action == "qa_record")
    );
    for field in ["task_id", "status", "summary", "issues", "ledger_path"] {
        assert!(properties.get(field).is_some(), "qa_record needs {field}");
    }
}

#[test]
fn cas_bb7e_worker_skill_allows_independent_qa_verdicts_in_every_flavor() {
    for flavor in ["", "codex/", "grok/"] {
        let details = load(&source_root().join(format!(
            "cas-cli/src/builtins/{flavor}skills/cas-worker/references/details.md"
        )));
        assert!(
            details.contains("verification action=qa_record"),
            "{flavor}: {details}"
        );
        assert!(details.contains("QA-pass task"), "{flavor}: {details}");
        assert!(
            !details.contains("Load only `task` and `coordination`;"),
            "{flavor}: {details}"
        );
    }
}

#[test]
// pin: Inspect source registration as well as supervisor references to detect a file omitted from the embedded catalog.
fn supervisor_reference_tree_uses_current_lifecycle_contract() {
    let root = source_root();
    // Audit D1: every flavor names tools by bare name.
    let flavors = ["", "codex/", "grok/"];

    for flavor in flavors {
        let base = root.join(format!("cas-cli/src/builtins/{flavor}skills"));

        let reference = load(&base.join("cas-supervisor/references/reference.md"));
        let model_selection = load(&base.join("cas-supervisor/references/model-selection.md"));

        assert_eq!(
            reference.matches("## Supervisor override").count(),
            1,
            "{flavor} reference.md must document supervisor_override once"
        );

        assert_eq!(
            model_selection.matches("suspended").count(),
            1,
            "{flavor} model-selection must keep one canonical suspension statement"
        );
        assert_eq!(
            model_selection.matches("## Spawn recipes").count(),
            1,
            "{flavor} model-selection must keep one recipe pointer"
        );
    }

    let builtins = include_str!("../src/builtins.rs");
    assert!(
        !builtins.contains("code-review-queue"),
        "builtins.rs must not register deleted code-review-queue.md"
    );

    // Audit D6 (cas-6b97): Codex ignores `.md` agents, so the inert
    // factory-supervisor agent is gone and its constraints live in the
    // Codex supervisor checklist.
    assert!(
        !builtins.contains("builtins/codex/agents/"),
        "builtins.rs must not register Codex .md agents"
    );
}
