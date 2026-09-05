use cas::builtins::{skill_catalog_for_harness, sync_all_builtins_for_harness};
use cas_mux::SupervisorCli;

const DIRECTIVE_PATH: &str = "skills/cas-supervisor/references/filing-cas-bugs.md";

// Keep this exhaustive list in sync with SupervisorCli and the exhaustive
// skill_catalog_for_harness dispatch. A newly shipped harness must opt in here
// so it cannot silently retain the lossy local-only bug intake directive.
const SHIPPED_HARNESSES: [SupervisorCli; 3] = [
    SupervisorCli::Claude,
    SupervisorCli::Codex,
    SupervisorCli::Grok,
];

fn assert_config_driven_issue_intake(harness: SupervisorCli, content: &str) {
    for required in [
        "[issues]",
        "repo = \"owner/repo\"",
        "cas config get issues.repo",
        "cas config set issues.repo",
        "command -v gh",
        "gh auth status",
        "gh issue create",
        "--repo",
    ] {
        assert!(
            content.contains(required),
            "{harness:?} filing directive is missing required intake guidance: {required}"
        );
    }

    assert!(
        content.contains("Do not derive") && content.contains("origin"),
        "{harness:?} filing directive must reject deriving the CAS issue target from a downstream origin"
    );
    assert!(
        content.contains("not installed") && content.contains("not authenticated"),
        "{harness:?} filing directive must preserve reports when gh is missing or unauthenticated"
    );
    let installation_specific_repo = ["pippenz", "cas"].join("/");
    assert!(
        !content.contains(&installation_specific_repo),
        "{harness:?} filing directive must never hardcode one user's repository"
    );
}

#[test]
fn every_shipped_harness_embeds_and_syncs_config_driven_issue_intake() {
    let mut canonical_directive = None;
    for harness in SHIPPED_HARNESSES {
        let embedded = skill_catalog_for_harness(harness)
            .iter()
            .find(|file| file.path == DIRECTIVE_PATH)
            .unwrap_or_else(|| panic!("{harness:?} catalog is missing {DIRECTIVE_PATH}"));
        assert_config_driven_issue_intake(harness, embedded.content);
        if let Some(canonical) = canonical_directive {
            assert_eq!(
                embedded.content, canonical,
                "{harness:?} filing directive drifted from the other shipped harnesses"
            );
        } else {
            canonical_directive = Some(embedded.content);
        }

        // A fresh, otherwise empty target models a downstream project that has
        // no cas-src checkout. Sync must still render the complete directive.
        let temp = tempfile::tempdir().expect("temp downstream project");
        sync_all_builtins_for_harness(harness, temp.path()).expect("sync builtins");
        let rendered = std::fs::read_to_string(temp.path().join(DIRECTIVE_PATH))
            .expect("read freshly synced filing directive");
        assert_eq!(rendered, embedded.content, "{harness:?} sync drifted");
        assert_config_driven_issue_intake(harness, &rendered);
    }
}

#[test]
fn every_issue_filing_builtin_names_the_component_registry() {
    for harness in SHIPPED_HARNESSES {
        for relative in [
            "skills/cas-worker/SKILL.md",
            "skills/cas-supervisor/SKILL.md",
            "skills/cas-github-issues/SKILL.md",
            DIRECTIVE_PATH,
        ] {
            let content = skill_catalog_for_harness(harness)
                .iter()
                .find(|file| file.path == relative)
                .unwrap_or_else(|| panic!("{harness:?} catalog is missing {relative}"))
                .content;
            for key in [
                "issues.repo",
                "issues.components.cassy",
                "issues.components.mecha_cassy",
                "issues.components.cloud",
            ] {
                assert!(
                    content.contains(key),
                    "{harness:?} {relative} is missing registry key {key}"
                );
            }
            assert!(
                content.contains("file a ticket in the matching repo before moving on"),
                "{harness:?} {relative} is missing the standing issue-filing directive"
            );
        }
    }
}
