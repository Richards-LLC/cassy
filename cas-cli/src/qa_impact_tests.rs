use super::*;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "impact fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "impact fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    git(repo.path(), &["init", "-q", "-b", "main"]);
    repo
}

fn commit(repo: &Path) -> String {
    git(repo, &["add", "."]);
    git(
        repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    );
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn qa_impact_orders_status_share_id_and_ignores_paths() {
    use clap::Parser;
    let cli = crate::cli::Cli::try_parse_from([
        "cas",
        "qa",
        "impact",
        "--files",
        "src/flow.ts",
        "--json",
    ])
    .unwrap();
    assert!(cli.json && matches!(cli.command, Some(crate::cli::Commands::Qa(_))));
    let post_release = crate::cli::Cli::try_parse_from([
        "cas",
        "qa",
        "post-release",
        "status",
        "--epic",
        "cas-1234",
    ])
    .unwrap();
    assert!(matches!(
        post_release.command,
        Some(crate::cli::Commands::Qa(_))
    ));
    let temp = repo();
    let repo = temp.path();
    write(repo, "src/flow.ts", "flow");
    write(repo, "src/flow.test.ts", "test");
    let mut yaml = String::from(
        "version: 1\nupdated: '2026-10-21'\nignore: ['**/*.test.ts']\nowner_groups:\n  core: {description: Core, globs: ['src/**']}\ninvariants:\n  INV: {statement: Arrived, check: Read state}\njourneys:\n",
    );
    for (id, status, share, owner) in [
        ("L-z", "live", "null", "owners: ['src/flow.ts']"),
        ("X-dead", "dead", "100", "owners: ['src/flow.ts']"),
        ("L-cold", "live", "10", "owners: ['src/flow.ts']"),
        ("L-hot", "live", "90", "owner_groups: [core]"),
        ("D-dark", "dark", "99", "owners: ['src/flow.ts']"),
        ("L-a", "live", "null", "owners: ['src/flow.ts']"),
        (
            "Only-ignored",
            "live",
            "100",
            "owners: ['src/flow.test.ts']",
        ),
    ] {
        yaml.push_str(&format!("  - id: {id}\n    status: {status}\n    status_reason: Flagged or historical\n    entry: {{url: /flow, trigger: Open}}\n    steps: [Arrive]\n    {owner}\n    invariants: [INV]\n    traffic: {{share_pct: {share}, basis: New accounts}}\n"));
    }
    write(repo, crate::qa_registry::DEFAULT_PATH, &yaml);
    let head = commit(repo);
    let report = analyze(
        repo,
        &QaConfig::default(),
        &[
            Change::modified("src/flow.ts"),
            Change::modified("src/flow.test.ts"),
        ],
        None,
        Some(&head),
    )
    .unwrap();
    assert_eq!(report.source, "registry");
    assert!(report.registry.valid);
    assert_eq!(
        report
            .journeys
            .iter()
            .map(|j| j.id.as_str())
            .collect::<Vec<_>>(),
        ["L-hot", "L-cold", "L-a", "L-z", "D-dark", "X-dead"]
    );
    assert_eq!(report.ignored, ["src/flow.test.ts"]);
    let first = &report.journeys[0];
    assert_eq!(
        (
            &*first.matches[0].file,
            &*first.matches[0].glob,
            &*first.matches[0].via
        ),
        ("src/flow.ts", "src/**", "owner_groups.core")
    );
    assert_eq!(
        first.evidence_required,
        "walk /flow on staging with a fresh persona; read back INV from the system of record; fixtures: none declared"
    );
    assert_eq!(
        report.journeys[4].evidence_required,
        "prove it stays unreachable"
    );
    assert_eq!(report.journeys[5].evidence_required, "none (NOT A ROUTE)");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["journeys"][0]["invariants"][0]["check"], "Read state");
}

#[test]
fn qa_impact_fixture_only_safety_never_sends_qa_to_staging() {
    let temp = repo();
    let repo = temp.path();
    write(repo, "src/deals.ts", "deals");
    write(
        repo,
        crate::qa_registry::DEFAULT_PATH,
        "version: 1\nupdated: '2026-10-22'\nsafety: {mode: fixture-only, forbidden_hosts: [quickbooks.api.intuit.com], protected_credentials: [QBO_REFRESH_TOKEN]}\ninvariants:\n  INV: {statement: Saved, check: Read state}\njourneys:\n  - id: deals\n    status: live\n    entry: {url: /deals, trigger: Open}\n    steps: [Arrive]\n    owners: ['src/**']\n    invariants: [INV]\n    traffic: {share_pct: null, basis: New accounts}\n",
    );
    let head = commit(repo);
    let report = analyze(
        repo,
        &QaConfig::default(),
        &[Change::modified("src/deals.ts")],
        None,
        Some(&head),
    )
    .unwrap();
    assert!(report.registry.valid, "{:?}", report.warnings);
    let note = crate::mcp::tools::core::task::lifecycle::qa_impact::render(
        &serde_json::to_value(&report).unwrap(),
    );
    assert!(
        note.contains("walk /deals against stubs; prove zero non-loopback requests"),
        "{note}"
    );
    assert!(
        note.contains("quickbooks.api.intuit.com") && note.contains("QBO_REFRESH_TOKEN"),
        "{note}"
    );
    assert!(
        !note.contains("staging") && !note.contains("system of record"),
        "{note}"
    );
}

#[test]
fn qa_impact_marks_a_planned_spec_as_planned_not_reviewable() {
    let temp = repo();
    let repo = temp.path();
    write(repo, "src/deals.ts", "deals");
    write(
        repo,
        crate::qa_registry::DEFAULT_PATH,
        "version: 1\nupdated: '2026-10-22'\ninvariants:\n  INV: {statement: Saved, check: Read state}\njourneys:\n  - id: deals\n    status: live\n    entry: {url: /deals, trigger: Open}\n    steps: [Arrive]\n    owners: ['src/**']\n    invariants: [INV]\n    spec: tests/deals.spec.ts\n    spec_status: planned\n",
    );
    let head = commit(repo);
    let report = analyze(
        repo,
        &QaConfig::default(),
        &[Change::modified("src/deals.ts")],
        None,
        Some(&head),
    )
    .unwrap();
    let journey = &report.journeys[0];
    assert_eq!(journey.spec, None);
    assert!(
        journey.evidence_required.ends_with(
            "; spec tests/deals.spec.ts is planned, not written: no automated coverage yet"
        ),
        "{}",
        journey.evidence_required
    );
}

#[test]
fn qa_impact_labels_unowned_with_longest_codemap_and_falls_back() {
    let temp = repo();
    let repo = temp.path();
    write(
        repo,
        ".claude/CODEMAP.md",
        "- `src/` - Broad area\n- `src/deep/` — Specific area\n",
    );
    commit(repo);
    for path in [
        "src/deep/page.vue",
        "src/deep/other.ts",
        "outside.txt",
        "src/owned.ts",
    ] {
        write(repo, path, "file");
    }
    commit(repo);
    let changes = [
        Change::modified("src/deep/page.vue"),
        Change::modified("src/deep/other.ts"),
        Change::modified("outside.txt"),
    ];
    let report = analyze(repo, &QaConfig::default(), &changes, None, None).unwrap();
    assert_eq!(report.source, "codemap_fallback");
    assert!(report.journeys.is_empty());
    assert_eq!(report.areas[0].prefix, "src/deep");
    assert_eq!(report.areas[0].files.len(), 2);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("No journey registry:"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("PRODUCT_OVERVIEW.md is missing:"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.starts_with("CODEMAP is stale ("))
    );
    write(
        repo,
        crate::qa_registry::DEFAULT_PATH,
        "version: 1\nupdated: '2026-10-21'\njourneys:\n  - id: owned\n    status: live\n    seeded: true\n    entry: {url: /, trigger: Open}\n    owners: ['src/owned.ts']\n",
    );
    let report = analyze(repo, &QaConfig::default(), &changes, None, None).unwrap();
    assert_eq!(report.source, "registry");
    assert!(report.areas.is_empty());
    let page = report
        .unowned
        .iter()
        .find(|p| p.file == "src/deep/page.vue")
        .unwrap();
    assert!(page.surface);
    assert_eq!(
        page.codemap.as_ref().unwrap(),
        &CodemapLabel {
            prefix: "src/deep".into(),
            summary: "Specific area".into()
        }
    );
    assert!(
        report
            .unowned
            .iter()
            .find(|p| p.file == "outside.txt")
            .unwrap()
            .codemap
            .is_none()
    );
}

#[test]
fn qa_impact_legacy_catalog_preserves_qa_pass_selection_at_head() {
    let temp = repo();
    let repo = temp.path();
    write(
        repo,
        "scripts/journeys-for-diff.py",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/journeys-for-diff.py"
        )),
    );
    write(
        repo,
        "docs/qa/journeys.md",
        "## hub-web\n\n- **Surface-wide:** `hub-web/src/*`\n\n### HUB-J1 · Reply\n\n- **Entry:** `/commander/`\n- **Touches:** `hub-web/src/reply.ts`\n- **Suite:** `hub-web/e2e/reply.journey.ts`\n\n### HUB-J2 · Theme\n\n- **Entry:** `/commander/`\n- **Touches:** `hub-web/src/theme.ts`\n- **Suite:** `hub-web/e2e/theme.journey.ts`\n",
    );
    for name in ["reply", "theme"] {
        write(
            repo,
            &format!("hub-web/src/{name}.ts"),
            "export const value = 'ok';",
        );
        write(
            repo,
            &format!("hub-web/e2e/{name}.journey.ts"),
            &format!(
                "test('HUB-J{} {name}', () => {{}});",
                if name == "reply" { 1 } else { 2 }
            ),
        );
    }
    let head = commit(repo);
    let paths = vec!["hub-web/src/reply.ts".to_owned()];
    let expected = crate::qa_pass::catalog_journeys_for(repo, Some(&head), &paths);
    assert_eq!(expected, ["HUB-J1"]);
    // The physical checkout's files must not override a delivered revision.
    write(repo, crate::qa_registry::DEFAULT_PATH, "invalid: [");
    write(repo, "docs/qa/journeys.md", "old checkout");
    let report = analyze(
        repo,
        &QaConfig::default(),
        &[Change::modified(&paths[0])],
        None,
        Some(&head),
    )
    .unwrap();
    assert_eq!(report.source, "legacy_catalog");
    assert_eq!(
        report
            .journeys
            .iter()
            .map(|j| j.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(report.journeys[0].name, "Reply");
    assert_eq!(
        report.journeys[0].spec.as_deref(),
        Some("hub-web/e2e/reply.journey.ts")
    );
}

#[test]
fn qa_impact_registry_preserves_catalog_imports_and_css_hunks() {
    let temp = repo();
    let repo = temp.path();
    write(
        repo,
        "scripts/journeys-for-diff.py",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../scripts/journeys-for-diff.py"
        )),
    );
    let mut catalog =
        String::from("## hub-web\n\n- **Surface-wide:** `hub-web/src/styles.css`\n\n");
    let mut yaml = String::from(
        "version: 1\nupdated: '2026-10-21'\nselection: legacy_catalog\ninvariants:\n  INV: {statement: Arrived, check: Read state}\njourneys:\n",
    );
    for (id, name) in [("HUB-J1", "reply"), ("HUB-J2", "theme")] {
        catalog.push_str(&format!("### {id} · {name}\n\n- **Entry:** `/commander/`\n- **Touches:** `hub-web/src/{name}.ts`\n- **Suite:** `hub-web/e2e/{name}.journey.ts`\n\n"));
        yaml.push_str(&format!("  - id: {id}\n    name: Curated {name}\n    status: live\n    entry: {{url: /commander/, trigger: Open}}\n    steps: [Arrive]\n    owners: ['hub-web/src/{name}.ts', 'hub-web/src/styles.css']\n    invariants: [INV]\n    traffic: {{share_pct: null}}\n    spec: hub-web/e2e/{name}.journey.ts\n"));
        write(
            repo,
            &format!("hub-web/src/{name}.ts"),
            &format!("export const value = '<div class=\"{name}-card\">ok</div>';"),
        );
        write(
            repo,
            &format!("hub-web/e2e/{name}.journey.ts"),
            &format!("test('{id} {name}', () => {{}});"),
        );
    }
    write(repo, "docs/qa/journeys.md", &catalog);
    write(repo, crate::qa_registry::DEFAULT_PATH, &yaml);
    write(repo, "hub-web/src/leaf.ts", "export const leaf = 1;");
    write(
        repo,
        "hub-web/src/reply.ts",
        "import './leaf'; export const value = '<div class=\"reply-card\">ok</div>';",
    );
    write(
        repo,
        "hub-web/src/styles.css",
        ".reply-card { color: blue; }\n.theme-card { color: green; }\n",
    );
    let base = commit(repo);
    write(
        repo,
        "hub-web/src/styles.css",
        ".reply-card { color: red; }\n.theme-card { color: green; }\n",
    );
    let head = commit(repo);
    // Both the selector and its data must come from the reviewed head.
    write(
        repo,
        "scripts/journeys-for-diff.py",
        "raise Exception('checkout drift')",
    );
    for path in ["hub-web/src/styles.css", "hub-web/src/leaf.ts"] {
        let report = analyze(
            repo,
            &QaConfig::default(),
            &[Change::modified(path)],
            Some(&base),
            Some(&head),
        )
        .unwrap();
        assert_eq!(report.source, "registry");
        assert!(report.registry.valid);
        assert_eq!(
            report
                .journeys
                .iter()
                .map(|j| j.id.as_str())
                .collect::<Vec<_>>(),
            ["HUB-J1"],
            "{path}"
        );
        assert_eq!(report.journeys[0].name, "Curated reply");
        assert_eq!(report.journeys[0].invariants[0].id, "INV");
    }
}

#[test]
fn qa_impact_commander_registry_matches_recent_merge_catalog_selection() {
    let source_root = crate::test_paths::workspace_root();
    let root = source_root.as_path();
    if !root.join(".git").exists() || !root.join("cas-cli/Cargo.toml").is_file() {
        eprintln!(
            "SKIP Commander registry parity: source checkout unavailable in this test archive"
        );
        return;
    }
    // Grow the catalog without refreshing the historical merge captures.
    // Keep the mutation in a private repository, including the selector's inputs.
    let snapshot = tempfile::tempdir().unwrap();
    git(
        root,
        &[
            "clone",
            "--quiet",
            "--shared",
            "--no-checkout",
            ".",
            snapshot.path().to_str().unwrap(),
        ],
    );
    let root = snapshot.path();
    git(
        root,
        &[
            "checkout",
            "HEAD",
            "--",
            "docs/qa",
            "hub-web",
            "scripts/journeys-for-diff.py",
        ],
    );
    let catalog = std::fs::read_to_string(root.join("docs/qa/journeys.md")).unwrap();
    let (_, template) = catalog.rsplit_once("\n### ").unwrap();
    let template_id = template.split(" · ").next().unwrap();
    let added_id = "HUB-J9999";
    write(
        root,
        "docs/qa/journeys.md",
        &format!("{catalog}\n### {}", template.replace(template_id, added_id)),
    );
    let mut registry =
        crate::qa_registry::load(&root.join(crate::qa_registry::DEFAULT_PATH)).unwrap();
    let mut added = registry
        .journeys
        .iter()
        .find(|j| j.id == template_id)
        .unwrap()
        .clone();
    added.id = added_id.into();
    registry.journeys.push(added);
    write(
        root,
        crate::qa_registry::DEFAULT_PATH,
        &serde_yaml_ng::to_string(&registry).unwrap(),
    );
    let head = commit(root);
    let validation =
        crate::qa_registry::check(root, Path::new(crate::qa_registry::DEFAULT_PATH)).unwrap();
    assert!(validation.problems.is_empty(), "{:?}", validation.problems);
    let registry = crate::qa_registry::load(&root.join(crate::qa_registry::DEFAULT_PATH)).unwrap();
    // pin: cas-src's enforcement must travel with its tracked registry.
    assert_eq!(
        registry.enforce,
        Some(crate::config::RegistryEnforcement::Enforce)
    );
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/commander-merge-diffs.json")).unwrap();
    let mut selected_source_merges = 0;
    let mut selected_added_journey = false;
    for diff in fixture["diffs"].as_array().unwrap() {
        // Replay real merge path sets against the migrated catalog at this head.
        // Original revisions are provenance, not a dependency on deep CI history.
        let files = diff["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let legacy =
            crate::qa_evidence::journeys::journeys_for_paths_at(root, &head, &files).unwrap();
        let report = analyze(
            root,
            &QaConfig::default(),
            &files.iter().map(Change::modified).collect::<Vec<_>>(),
            None,
            Some(&head),
        )
        .unwrap();
        assert_eq!(report.source, "registry");
        assert!(report.registry.valid);
        let ids = report
            .journeys
            .iter()
            .map(|j| j.id.as_str())
            .collect::<Vec<_>>();
        selected_added_journey |= ids.contains(&added_id);
        if !ids.is_empty()
            && files
                .iter()
                .any(|file| file.starts_with("hub-web/src/") && !file.ends_with(".test.ts"))
        {
            selected_source_merges += 1;
        }
        assert_eq!(ids, legacy, "merge {}", diff["head"]);
        assert_eq!(
            serde_json::to_value(ids).unwrap(),
            diff["journeys"],
            "captured legacy IDs for {}",
            diff["head"]
        );
    }
    assert!(
        selected_source_merges > 0,
        "parity needs a production source merge that selects journeys"
    );
    assert!(
        selected_added_journey,
        "parity must exercise catalog growth"
    );
}

/// GH #1340: qa impact, `cas codemap status` and `cas qa journeys --status`
/// share one CODEMAP freshness source. A regen commit that also adds files,
/// plus untracked checkout debris, is fresh in all three; a later structural
/// commit makes all three stale.
#[test]
fn codemap_freshness_agrees_across_qa_impact_status_and_journeys_gh_1340() {
    use crate::hooks::handlers::handlers_events::{
        codemap::{CodemapStaleness, evaluate_codemap_at},
        docs_freshness::{Measurement, UpkeepReport},
    };
    let temp = repo();
    let repo = temp.path();
    let origin = tempfile::tempdir().unwrap();
    git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
    git(
        repo,
        &["remote", "add", "origin", &origin.path().to_string_lossy()],
    );
    write(repo, ".claude/CODEMAP.md", "- `src/` - Source\n");
    commit(repo);
    write(
        repo,
        ".claude/CODEMAP.md",
        "- `src/` - Source\n- `docs/` - Docs\n",
    );
    write(repo, "docs/PRODUCT_OVERVIEW.md", "overview");
    write(repo, "docs/qa/notes.md", "notes");
    commit(repo);
    let publish = || {
        git(repo, &["push", "-q", "origin", "main"]);
        git(repo, &["fetch", "-q", "origin"]);
        git(repo, &["remote", "set-head", "origin", "main"]);
    };
    publish();
    write(repo, ".playwright-mcp/page.yml", "untracked debris");
    let store = repo.join(".cas");
    let counts = || {
        let impact = analyze(
            repo,
            &QaConfig::default(),
            &[Change::modified("src/a.ts")],
            None,
            None,
        )
        .unwrap()
        .warnings
        .iter()
        .find_map(|w| {
            w.strip_prefix("CODEMAP is stale (")
                .map(|rest| rest.split(' ').next().unwrap().to_owned())
        });
        let status = match evaluate_codemap_at(&Measurement::at(repo, &store))
            .unwrap()
            .staleness
        {
            None => Some(0),
            Some(CodemapStaleness::Stale { total_changes, .. }) => Some(total_changes),
            other => panic!("unexpected status {other:?}"),
        };
        let journeys = UpkeepReport::at(
            &Measurement::integration_at(repo, &store),
            &QaConfig::default(),
        )
        .codemap
        .drift_count;
        (impact, status, journeys)
    };
    assert_eq!(counts(), (None, Some(0), Some(0)));
    // `commit` stages everything; drop the debris so only src/new.ts lands.
    std::fs::remove_dir_all(repo.join(".playwright-mcp")).unwrap();
    write(repo, "src/new.ts", "new");
    commit(repo);
    publish();
    assert_eq!(counts(), (Some("1".to_owned()), Some(1), Some(1)));
}
