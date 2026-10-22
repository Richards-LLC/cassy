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
    assert!(first.evidence_required.contains("INV"));
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
    let head = git(root, &["rev-parse", "HEAD"]);
    let mut selected_source_merges = 0;
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
}

/// cas-d863 (+ cas-9779 carry-over): G8 reads only the verdict column, found
/// by its header (Verdict, Run, Result or Status) in every documented ledger
/// format. A BLOCKED_EXTERNAL journey row stays owed and names its provider,
/// and a keyed journey counts distinct run identities, refusing duplicate or
/// unidentified runs by name; an unkeyed journey beside it needs no identity.
#[test]
fn g8_ledger_reads_verdict_column_owes_blocked_rows_and_counts_independent_runs() {
    use crate::mcp::tools::core::task::lifecycle::qa_impact::ledger_refusal;
    let temp = repo();
    let repo = temp.path();
    write(repo, "src/audit.ts", "audit");
    let mut yaml = String::from(
        "version: 1\nupdated: '2026-10-22'\ninvariants:\n  INV: {statement: Audited, check: Read the run}\njourneys:\n",
    );
    for (id, key) in [
        ("AUDIT", "\n    independence_key: post_audit_v2_runs.id"),
        ("SEND", ""),
    ] {
        yaml.push_str(&format!("  - id: {id}\n    status: live\n    entry: {{url: /audit, trigger: Audit}}\n    steps: [Submit]\n    owners: ['src/audit.ts']\n    invariants: [INV]\n    traffic: {{share_pct: 50, basis: audits}}{key}\n"));
    }
    write(repo, crate::qa_registry::DEFAULT_PATH, &yaml);
    let head = commit(repo);
    let report = analyze(
        repo,
        &QaConfig::default(),
        &[Change::modified("src/audit.ts")],
        None,
        Some(&head),
    )
    .unwrap();
    assert!(report.registry.valid, "{:?}", report.registry);
    let mut impact = serde_json::to_value(&report).unwrap();
    impact["head_sha"] = head.clone().into();
    let refusal = |ledger: String| ledger_refusal(&impact, &head, &ledger);
    let grammar = |rows: &[&str]| {
        let mut ledger = String::from(
            "| id | cell | expected | observed | verdict | label | evidence | defect |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for row in rows {
            ledger.push_str(row);
            ledger.push('\n');
        }
        ledger
            .push_str("| SEND | composer | sent | sent | PASS | real-build | qa/send.png | - |\n");
        ledger
    };

    let blocked = refusal(grammar(&[
        "| AUDIT | instagram url | audited | FAIL: 502 | BLOCKED_EXTERNAL | real-build | blocked: provider=rapidapi error=502 failed_fetch | - |",
    ]))
    .expect("a blocked row is owed, not a verdict");
    assert!(
        blocked.contains("AUDIT (cell instagram url) is BLOCKED_EXTERNAL{provider=rapidapi, error=502 failed_fetch}"),
        "{blocked}"
    );
    let observed = refusal(grammar(&[
        "| AUDIT | url | audited | FAIL: 502 | partial | real-build | - | - |",
    ]))
    .expect("a verdict word outside the verdict column is not a verdict");
    assert!(
        observed.contains("no row for live journeys AUDIT."),
        "{observed}"
    );
    let run = |id: &str| {
        format!(
            "| AUDIT | run | audited | post_audit_v2_runs.id={id} | PASS | real-build | - | - |"
        )
    };
    let duplicate = refusal(grammar(&[&run("a1"), &run("b2"), &run("b2")]))
        .expect("a duplicate run identity is not an independent run");
    assert!(
        duplicate.contains("3 AUDIT runs recorded, 2 independent")
            && duplicate.contains("duplicate post_audit_v2_runs.id=b2"),
        "{duplicate}"
    );
    let unidentified = refusal(grammar(&[
        &run("a1"),
        "| AUDIT | run | audited | done | FAIL | real-build | - | - |",
    ]))
    .expect("a run without an identity is not independent");
    assert!(
        unidentified.contains("1 without post_audit_v2_runs.id=<id>"),
        "{unidentified}"
    );
    assert_eq!(
        refusal(grammar(&[&run("a1"), &run("b2"), &run("c3")])),
        None
    );
    // Documented formats: the independent-QA journeys table (PASS in `Run`),
    // scripts/journey-bundles.py, and short Result/Status tables. Real ledgers
    // (cas-4916) also use `Run` for run ids beside `Result`, compound headers
    // and bold verdicts.
    for ledger in [
        "| Journey | Result | Run |\n| --- | --- | --- |\n| AUDIT | PASS | post_audit_v2_runs.id=a1 |\n| AUDIT | FAIL | post_audit_v2_runs.id=b2 |\n| SEND | PASS | 1 |\n".to_string(),
        "| ID | Verdict / label | Evidence |\n| --- | --- | --- |\n| AUDIT | PASS / real-build | post_audit_v2_runs.id=a1 |\n| SEND | NOT A ROUTE / source | - |\n".to_string(),
        "| Journey | Verdict |\n| --- | --- |\n| AUDIT | **PASS** post_audit_v2_runs.id=a1 |\n| SEND | **NOT EXERCISED:** sandbox unavailable |\n".to_string(),
        "| ID | Run | Dead end | Copy | Steps | Context | Waits | Severity | Findings / tasks |\n|---|---|---|---|---|---|---|---|---|\n| AUDIT | PASS | 0 | 1 | 0 | 0 | 0 | none | post_audit_v2_runs.id=a1 |\n| SEND | FAIL | 1 | 0 | 0 | 0 | 0 | high | cas-1 |\n".to_string(),
        "| ID | Journey | Run | Total | Slowest stage |\n|---|---|---|---|---|\n| AUDIT | Audit post_audit_v2_runs.id=a1 | PASS | 4.2s | submit (3.1s) |\n| SEND | Send | PASS | 1.0s | send (0.4s) |\n".to_string(),
        "| Journey | Result |\n| --- | --- |\n| AUDIT | PASS post_audit_v2_runs.id=a1 |\n| SEND | NOT EXERCISED: sandbox unavailable |\n".to_string(),
        "| Journey | Status | Note |\n| --- | --- | --- |\n| AUDIT | PASS | post_audit_v2_runs.id=a1 |\n| SEND | NOT A ROUTE | retired |\n".to_string(),
    ] {
        assert_eq!(refusal(ledger.clone()), None, "{ledger}");
    }
}
