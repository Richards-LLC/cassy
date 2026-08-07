//! cas-f4c1 (M3 of EPIC cas-b129) — acceptance tests for the legacy
//! memory → knowledge-page migration.
//!
//! Written test-first against `docs/migration/cas-b129-mapping-spec.md`, which
//! is normative. Every test below names the spec clause or acceptance criterion
//! it defends, so a future edit that breaks a rule fails with a pointer to the
//! paragraph it violated.

use std::path::Path;

use cas::memory_migration::{
    self, DbLabel, Disposition, MigrationConfig, SourceDb, frontmatter, routing, source,
};
use cas_store::{ENTRIES_RULES_SCHEMA, KnowledgeStore, SqliteKnowledgeStore};
use rusqlite::params;
use tempfile::TempDir;

// ── fixtures ────────────────────────────────────────────────────────────

/// Create an initialized legacy `.cas` root and return its path.
fn legacy_root(temp: &TempDir, name: &str) -> std::path::PathBuf {
    let root = temp.path().join(name);
    std::fs::create_dir_all(&root).unwrap();
    // `SqliteStore::open` does not run DDL (schema init lives in `store_init`),
    // so apply the shipped entries schema directly.
    rusqlite::Connection::open(root.join("cas.db"))
        .unwrap()
        .execute_batch(ENTRIES_RULES_SCHEMA)
        .unwrap();
    root
}

fn raw(root: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join("cas.db")).unwrap()
}

/// Minimal row insert. Everything not named takes the DDL default, which is
/// exactly what Rule C3 (omit-when-default) is specified against.
fn insert_row(conn: &rusqlite::Connection, id: &str, ty: &str, title: Option<&str>, content: &str) {
    conn.execute(
        "INSERT INTO entries (id, type, created, title, content) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, ty, "2026-01-01T00:00:00Z", title, content],
    )
    .unwrap();
}

fn config(
    roots: Vec<(DbLabel, std::path::PathBuf)>,
    ledger: &Path,
    apply: bool,
) -> MigrationConfig {
    MigrationConfig {
        sources: roots
            .into_iter()
            .map(|(label, root)| SourceDb {
                label,
                db_path: root.join("cas.db"),
                cas_root: root,
            })
            .collect(),
        ledger_dir: ledger.to_path_buf(),
        apply,
        invalidate_sync_queue: false,
        page_size: 500,
        reindex: false,
        stop_after: None,
    }
}

// ── §4.1 / E1 as amended — contamination predicate ──────────────────────

#[test]
fn digit_tokens_match_only_as_standalone_words() {
    // Supervisor amendment to E1: a cas-src memory containing `file.rs:1040`
    // must NOT quarantine, but a real Accounting record must.
    assert_eq!(
        routing::contamination_token("", "see cas-cli/src/lib.rs:1040"),
        None
    );
    assert_eq!(
        routing::contamination_token("", "line1040 of the buffer"),
        None
    );
    assert_eq!(routing::contamination_token("", "v1.1065-beta"), None);
    assert_eq!(routing::contamination_token("", "index 1040-1065"), None);

    assert_eq!(
        routing::contamination_token("Richards 1040 — All three years complete", ""),
        Some("1040".to_string())
    );
    assert_eq!(
        routing::contamination_token("", "filed the 1065 return."),
        Some("1065".to_string())
    );
    assert_eq!(
        routing::contamination_token("", "(1040) attached"),
        Some("1040".to_string())
    );
}

#[test]
fn proper_noun_tokens_match_as_substrings_case_sensitively() {
    for token in ["QBO", "TNTAP", "FONCE", "FAE 183", "Journal Entr"] {
        let haystack = format!("prefix{token}suffix");
        assert_eq!(
            routing::contamination_token("", &haystack),
            Some(token.to_string()),
            "{token} must match as a substring"
        );
        // Case-sensitive: these are proper nouns and form numbers.
        assert_eq!(
            routing::contamination_token("", &haystack.to_lowercase()),
            None,
            "{token} must be case-sensitive"
        );
    }
}

// ── §4 R1 — fixture predicate is exact equality, never LIKE ─────────────

#[test]
fn fixture_predicate_is_exact_equality_not_substring() {
    let exact = routing::FIXTURE_CONTENTS[0];
    let row = source::LegacyRow::for_test("g-1", "learning", None, exact);
    assert_eq!(routing::route(&row).unwrap().rule, "R1");

    let near = format!("{exact} plus a real observation about the parser");
    let row = source::LegacyRow::for_test("g-2", "learning", None, &near);
    assert_ne!(
        routing::route(&row).unwrap().rule,
        "R1",
        "a superstring of a fixture string is a real memory and must not be dropped"
    );
}

// ── §4 — the decision procedure is ordered and total ────────────────────

#[test]
fn routing_is_ordered_and_total() {
    let cases: Vec<(source::LegacyRow, &str, Disposition)> = vec![
        (
            source::LegacyRow::for_test("a", "learning", None, routing::FIXTURE_CONTENTS[2]),
            "R1",
            Disposition::DeliberatelyLeave,
        ),
        (
            source::LegacyRow::for_test("b", "learning", None, "pinned fact")
                .with_memory_tier("in_context"),
            "R2",
            Disposition::CarryVerbatim,
        ),
        (
            source::LegacyRow::for_test("c", "preference", None, "naming taste"),
            "R3",
            Disposition::CarryVerbatim,
        ),
        (
            source::LegacyRow::for_test("d", "learning", None, "maybe")
                .with_belief("hypothesis", 0.4),
            "R4",
            Disposition::StayEntry,
        ),
        (
            source::LegacyRow::for_test("e", "observation", None, "ran the tests"),
            "R5",
            Disposition::StayEntry,
        ),
        (
            source::LegacyRow::for_test("f", "context", None, "QBO reconciliation done"),
            "R6",
            Disposition::DeliberatelyLeave,
        ),
        (
            source::LegacyRow::for_test("g", "context", None, "the hooks live in cas-core"),
            "R7",
            Disposition::MigrateToPage,
        ),
        (
            source::LegacyRow::for_test("h", "learning", None, "always export ZIG")
                .with_feedback(3, 1),
            "R8",
            Disposition::MigrateToPage,
        ),
        (
            source::LegacyRow::for_test("i", "learning", None, "one-off session note"),
            "R9",
            Disposition::StayEntry,
        ),
    ];
    for (row, rule, disposition) in cases {
        let route = routing::route(&row).unwrap();
        assert_eq!(route.rule, rule, "row {} routed to {}", row.id, route.rule);
        assert_eq!(route.disposition, disposition);
    }
}

#[test]
fn ordering_matters_pin_beats_preference_and_fixture_beats_everything() {
    // R2 (pin) is evaluated before R3 (preference): both carry-verbatim, but
    // the audit must attribute the row to the pin rule.
    let row = source::LegacyRow::for_test("x", "preference", None, "pinned pref")
        .with_memory_tier("in_context");
    assert_eq!(routing::route(&row).unwrap().rule, "R2");

    // R1 (fixture) precedes the contamination quarantine and everything else.
    let row = source::LegacyRow::for_test("y", "preference", None, routing::FIXTURE_CONTENTS[4])
        .with_memory_tier("in_context");
    assert_eq!(routing::route(&row).unwrap().rule, "R1");
}

#[test]
fn an_unrouted_row_is_a_hard_error() {
    // §11 assert 7. A type outside EntryType must abort the run, never be
    // silently skipped into the "unaccounted" bucket.
    let row = source::LegacyRow::for_test("z", "telepathy", None, "unknown type");
    let err = routing::route(&row).unwrap_err().to_string();
    assert!(
        err.contains("telepathy"),
        "error names the offending value: {err}"
    );
}

// ── §2 C2/C3/C4 — the frontmatter carrier ───────────────────────────────

#[test]
fn omit_when_default_keeps_untouched_columns_out_of_the_frontmatter() {
    let row = source::LegacyRow::for_test("g-1", "learning", None, "body");
    let lines = frontmatter::legacy_lines(&row, DbLabel::Global);
    let joined = lines.join("\n");
    // Defaults per C3 — none of these may be emitted.
    for absent in [
        "cas_legacy_type:",
        "cas_legacy_archived:",
        "cas_legacy_stability:",
        "cas_legacy_importance:",
        "cas_legacy_access_count:",
        "cas_legacy_memory_tier:",
        "cas_legacy_helpful_count:",
        "cas_legacy_harmful_count:",
        "cas_legacy_belief_type:",
        "cas_legacy_confidence:",
        "cas_legacy_session_id:",
        "cas_legacy_domain:",
        "cas_legacy_branch:",
    ] {
        assert!(
            !joined.contains(absent),
            "{absent} is a default and must be omitted:\n{joined}"
        );
    }
    // Identity keys are never defaulted away.
    assert!(joined.contains("cas_legacy_id: g-1"));
    assert!(joined.contains("cas_legacy_db: global"));
    assert!(joined.contains("cas_legacy_scope: global"));
    assert!(joined.contains("cas_legacy_created: 2026-01-01T00:00:00Z"));
}

#[test]
fn every_emitted_key_is_a_reserved_cas_legacy_key() {
    // C2 says the reserved set is complete: M3 emits no others.
    let row = source::LegacyRow::fully_populated_for_test();
    for line in frontmatter::legacy_lines(&row, DbLabel::Project) {
        if line.trim_start().starts_with("- ") {
            continue; // the one list block (cas_legacy_tags)
        }
        let key = line.split_once(':').expect("flat scalar line").0.trim();
        assert!(
            frontmatter::RESERVED_KEYS.contains(&key),
            "{key} is not a reserved cas_legacy_* key"
        );
    }
}

#[test]
fn frontmatter_survives_a_parse_render_parse_round_trip() {
    // C4: `parse_frontmatter` is a hand-rolled line reader, not YAML. Anything
    // we emit must come back byte-for-byte through the passthrough branch.
    let row = source::LegacyRow::fully_populated_for_test();
    let body = memory_migration::compose_body(&row, DbLabel::Project, "Some Title", false);

    let first = cas::knowledge::merge::parse_frontmatter(&body);
    let rendered = cas::knowledge::merge::render_frontmatter(&first);
    let second = cas::knowledge::merge::parse_frontmatter(&rendered);
    assert_eq!(
        first.passthrough, second.passthrough,
        "passthrough is not stable"
    );
    assert!(
        first
            .passthrough
            .iter()
            .any(|l| l.starts_with("cas_legacy_id:")),
        "legacy keys landed outside passthrough: {:?}",
        first.passthrough
    );
    // Values that contain a colon or newline must not corrupt the block.
    assert_eq!(first.passthrough.len(), second.passthrough.len());
}

#[test]
fn multiline_values_cannot_break_the_frontmatter_block() {
    let mut row = source::LegacyRow::for_test("p-1", "context", Some("t"), "body");
    row.source_tool = Some("mcp\nmalicious: true".to_string());
    let lines = frontmatter::legacy_lines(&row, DbLabel::Project);
    assert!(
        lines.iter().all(|l| !l.contains('\n')),
        "a value smuggled a newline into the block: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.trim_start().starts_with("malicious")),
        "a value forged a frontmatter key"
    );
}

// ── AC1 — the loss audit accounts for every legacy row ──────────────────

#[test]
fn dry_run_audit_accounts_for_every_legacy_row() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        for (i, fixture) in routing::FIXTURE_CONTENTS.iter().enumerate() {
            insert_row(&conn, &format!("p-fix{i}"), "learning", None, fixture);
        }
        insert_row(
            &conn,
            "p-ctx",
            "context",
            Some("Hooks"),
            "hooks live in cas-core",
        );
        insert_row(
            &conn,
            "p-pref",
            "preference",
            Some("Naming"),
            "prefer short names",
        );
        insert_row(&conn, "p-obs", "observation", None, "ran the suite");
        insert_row(&conn, "p-learn", "learning", None, "a session learning");
        insert_row(
            &conn,
            "p-dirty",
            "context",
            Some("QBO state"),
            "44 JEs received",
        );
    }
    let ledger = temp.path().join("ledger");
    let out =
        memory_migration::run(&config(vec![(DbLabel::Project, root)], &ledger, false)).unwrap();

    assert_eq!(out.audit.total, 10);
    assert_eq!(out.audit.unaccounted, 0);
    assert!(out.audit.balances(), "buckets do not sum to the row total");
    assert_eq!(out.audit.deliberately_left, 6); // 5 fixtures + 1 quarantine
    assert_eq!(out.audit.migrated, 1); // p-ctx
    assert_eq!(out.audit.carried_verbatim, 1); // p-pref
    assert_eq!(out.audit.stay_entry, 2); // p-obs, p-learn
    assert_eq!(out.audit.merged_into, 0);

    let table = out.audit.render_table();
    assert!(
        table.contains("unaccounted"),
        "audit table must show the unaccounted line"
    );
    assert!(table.contains("R1"), "audit table must break down per rule");
}

#[test]
fn dry_run_writes_no_pages_and_no_body_files() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(
            &conn,
            "p-ctx",
            "context",
            Some("Hooks"),
            "hooks live in cas-core",
        );
        insert_row(
            &conn,
            "p-pref",
            "preference",
            Some("Naming"),
            "prefer short names",
        );
    }
    let ledger = temp.path().join("ledger");
    let out = memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        false,
    ))
    .unwrap();
    assert_eq!(out.applied, 0);

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    assert!(
        store.list_pages().unwrap().is_empty(),
        "dry run wrote pages"
    );
    assert!(
        !root.join("knowledge").exists()
            || std::fs::read_dir(root.join("knowledge")).unwrap().count() == 0
    );
}

#[test]
fn dry_run_prints_the_full_quarantine_list() {
    // Supervisor ruling on E1: the dry run MUST print id + title + matched
    // token for every quarantined row before any apply is authorized.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(&conn, "p-q1", "context", Some("FINAL QBO State"), "44 JEs");
        insert_row(
            &conn,
            "p-q2",
            "context",
            Some("Roark 2023"),
            "FAE 183 FONCE submitted",
        );
    }
    let ledger = temp.path().join("ledger");
    let out =
        memory_migration::run(&config(vec![(DbLabel::Project, root)], &ledger, false)).unwrap();

    assert_eq!(out.quarantine.len(), 2);
    let report = memory_migration::render_quarantine(&out.quarantine);
    assert!(
        report.contains("p-q1") && report.contains("FINAL QBO State") && report.contains("QBO")
    );
    assert!(report.contains("p-q2") && report.contains("Roark 2023"));
    // Quarantine is stay-entry-in-place plus a ledger file — never a delete.
    assert!(ledger.join("quarantine.jsonl").exists());
}

// ── AC2 — idempotence ───────────────────────────────────────────────────

#[test]
fn apply_then_reapply_produces_identical_state() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(
            &conn,
            "p-ctx",
            "context",
            Some("Hooks"),
            "hooks live in cas-core",
        );
        insert_row(
            &conn,
            "p-pref",
            "preference",
            Some("Naming"),
            "prefer short names",
        );
        insert_row(
            &conn,
            "p-ctx2",
            "context",
            None,
            "a memory with no title at all",
        );
    }
    let ledger = temp.path().join("ledger");
    let cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);

    let first = memory_migration::run(&cfg).unwrap();
    assert_eq!(first.applied, 3);
    let snapshot_one = knowledge_snapshot(&root);

    let second = memory_migration::run(&cfg).unwrap();
    assert_eq!(second.applied, 0, "re-apply must be a no-op");
    assert_eq!(second.skipped_already_applied, 3);
    let snapshot_two = knowledge_snapshot(&root);

    assert_eq!(snapshot_one, snapshot_two, "re-apply changed state");
    assert_eq!(snapshot_two.len(), 3, "3 legacy rows must yield 3 pages");
}

#[test]
fn colliding_titles_do_not_collapse_into_one_page() {
    // Trap: canonical_rel_path is UNIQUE and ~2/3 of legacy rows fall back to
    // preview(60) titles, so canonical paths would ON-CONFLICT-overwrite two
    // distinct memories. Every migrated page therefore carries a legacy-id
    // suffix in its rel_path.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(
            &conn,
            "p-a",
            "context",
            Some("Same Title"),
            "first distinct memory",
        );
        insert_row(
            &conn,
            "p-b",
            "context",
            Some("Same Title"),
            "second distinct memory",
        );
    }
    let ledger = temp.path().join("ledger");
    let out = memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();
    assert_eq!(out.applied, 2);
    assert_eq!(
        out.audit.merged_into, 0,
        "a collision silently merged two rows"
    );

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let pages = store.list_pages().unwrap();
    assert_eq!(pages.len(), 2);
    let bodies: Vec<String> = pages
        .iter()
        .map(|p| store.read_body(&p.rel_path).unwrap())
        .collect();
    assert!(bodies.iter().any(|b| b.contains("first distinct memory")));
    assert!(bodies.iter().any(|b| b.contains("second distinct memory")));
}

// ── AC3 — resumability ──────────────────────────────────────────────────

#[test]
fn resume_after_a_partial_apply_completes_with_a_clean_audit() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        for i in 0..6 {
            insert_row(
                &conn,
                &format!("p-{i}"),
                "context",
                Some(&format!("Page {i}")),
                &format!("content {i}"),
            );
        }
    }
    let ledger = temp.path().join("ledger");
    let mut cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);

    // Simulate a crash: stop after 2 rows, exactly as an interrupted process
    // would leave the ledger.
    cfg.stop_after = Some(2);
    let partial = memory_migration::run(&cfg).unwrap();
    assert_eq!(partial.applied, 2);

    cfg.stop_after = None;
    let resumed = memory_migration::run(&cfg).unwrap();
    assert_eq!(resumed.applied, 4, "resume must continue, not restart");
    assert_eq!(resumed.skipped_already_applied, 2);
    assert_eq!(resumed.audit.unaccounted, 0);
    assert!(resumed.audit.balances());

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    assert_eq!(store.list_pages().unwrap().len(), 6);
}

#[test]
fn resume_is_keyed_on_legacy_id_not_row_order() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(&conn, "p-mmm", "context", Some("Middle"), "m");
        insert_row(&conn, "p-zzz", "context", Some("Last"), "z");
    }
    let ledger = temp.path().join("ledger");
    let mut cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);
    cfg.stop_after = Some(1);
    memory_migration::run(&cfg).unwrap();

    // A row inserted *before* the resume, sorting ahead of everything already
    // applied, must not shift the resume window.
    insert_row(&raw(&root), "p-aaa", "context", Some("First"), "a");
    cfg.stop_after = None;
    let resumed = memory_migration::run(&cfg).unwrap();
    assert_eq!(resumed.skipped_already_applied, 1);
    assert_eq!(resumed.applied, 2);
}

#[test]
fn a_locked_page_written_before_a_crash_does_not_wedge_the_resume() {
    // The dangerous interleaving: carry-verbatim page written AND locked, then
    // the ledger line is lost. commit_ingest refuses to update a locked page
    // (`WHERE locked = 0`), so a naive resume would report zero pages written
    // and fail forever.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(
        &raw(&root),
        "p-pref",
        "preference",
        Some("Naming"),
        "prefer short names",
    );
    let ledger = temp.path().join("ledger");
    let cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);

    memory_migration::run(&cfg).unwrap();
    // Erase the ledger to mimic a crash between the lock and the ledger flush.
    std::fs::write(ledger.join("applied.jsonl"), "").unwrap();

    let again = memory_migration::run(&cfg).unwrap();
    assert_eq!(
        again.applied, 1,
        "resume must be able to rewrite its own locked page"
    );
    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let pages = store.list_pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert!(pages[0].locked, "the page must end the run locked again");
}

// ── AC4 — carry-verbatim is byte-identical and locked ───────────────────

#[test]
fn carry_verbatim_bodies_are_byte_identical_and_locked() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    let tricky = "Naming taste:\n\n---\n\n  * prefer `short` names\n\ttrailing tab\t\n";
    {
        let conn = raw(&root);
        insert_row(&conn, "p-pref", "preference", Some("Naming taste"), tricky);
        conn.execute(
            "INSERT INTO entries (id, type, created, title, content, memory_tier) VALUES (?1,'learning',?2,?3,?4,'in_context')",
            params!["p-pin", "2026-01-01T00:00:00Z", "Pinned", "pinned body\n"],
        )
        .unwrap();
    }
    let ledger = temp.path().join("ledger");
    memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let pages = store.list_pages().unwrap();
    assert_eq!(pages.len(), 2);
    for page in &pages {
        assert!(
            page.locked,
            "carry-verbatim page {} is not locked (L1)",
            page.rel_path
        );
        let body = store.read_body(&page.rel_path).unwrap();
        let carried = memory_migration::body_after_frontmatter(&body);
        let expected = if page.title == "Pinned" {
            "pinned body\n"
        } else {
            tricky
        };
        assert_eq!(
            carried, expected,
            "body is not byte-identical for {}",
            page.rel_path
        );
        assert!(
            body.contains("locked: true"),
            "frontmatter lock bit missing"
        );
    }
}

#[test]
fn migrate_to_page_rows_are_left_unlocked() {
    // L2 — the whole point of the page/entry split: distillation may improve
    // a migrated page later.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(
        &raw(&root),
        "p-ctx",
        "context",
        Some("Hooks"),
        "hooks live in cas-core",
    );
    let ledger = temp.path().join("ledger");
    memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let pages = store.list_pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert!(!pages[0].locked, "a migrate-to-page row must stay unlocked");
}

// ── AC5 — provenance and team-share carry ───────────────────────────────

#[test]
fn provenance_and_team_id_are_carried_and_share_is_never_synthesized() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        conn.execute(
            "INSERT INTO entries
               (id, type, created, updated_at, title, content, helpful_count, harmful_count,
                access_count, last_accessed, importance, stability, team_id, tags, source_tool)
             VALUES (?1,'context',?2,?3,?4,?5,7,2,42,?6,0.9,0.75,?7,?8,'mcp')",
            params![
                "p-prov",
                "2024-03-04T05:06:07Z",
                "2025-09-08T09:10:11Z",
                "Provenance",
                "a durable project fact",
                "2025-10-01T00:00:00Z",
                "team-42",
                "alpha,beta"
            ],
        )
        .unwrap();
    }
    let ledger = temp.path().join("ledger");
    memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let page = store.list_pages().unwrap().remove(0);
    // P1 — native columns carry the legacy timestamps.
    assert_eq!(page.created_at.to_rfc3339(), "2024-03-04T05:06:07+00:00");
    assert_eq!(page.updated_at.to_rfc3339(), "2025-09-08T09:10:11+00:00");
    // P2 — sources are empty; a synthetic path would be clobbered by the next
    // distillation pass and would corrupt the source ledger.
    assert!(page.sources.is_empty(), "sources_json must be []");

    let body = store.read_body(&page.rel_path).unwrap();
    for expected in [
        "cas_legacy_id: p-prov",
        "cas_legacy_db: project",
        "cas_legacy_helpful_count: 7",
        "cas_legacy_harmful_count: 2",
        "cas_legacy_access_count: 42",
        "cas_legacy_last_accessed: 2025-10-01T00:00:00Z",
        "cas_legacy_importance: 0.9",
        "cas_legacy_stability: 0.75",
        "cas_legacy_team_id: team-42",
        "cas_legacy_created: 2024-03-04T05:06:07Z",
        "cas_legacy_updated_at: 2025-09-08T09:10:11Z",
    ] {
        assert!(body.contains(expected), "missing {expected} in:\n{body}");
    }
    // Tags ride as the single list block permitted by C4.
    assert!(
        body.contains("cas_legacy_tags:\n  - alpha\n  - beta"),
        "tags list block:\n{body}"
    );
    // S1 — share is deliberately left; nothing may synthesize it.
    assert!(
        !body.contains("cas_legacy_share"),
        "share must never be emitted"
    );
    assert!(!body.contains("share:"), "share must never be synthesized");
}

#[test]
fn scope_is_derived_not_read_from_the_column() {
    // §5.12 — all 450 GLOBAL rows claim `scope='project'`, so the column is
    // known-false and must never be carried.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "global");
    {
        let conn = raw(&root);
        conn.execute(
            "INSERT INTO entries (id, type, created, title, content, scope) VALUES (?1,'context',?2,?3,?4,'project')",
            params!["g-1", "2026-01-01T00:00:00Z", "Global fact", "a global fact"],
        )
        .unwrap();
    }
    let ledger = temp.path().join("ledger");
    memory_migration::run(&config(
        vec![(DbLabel::Global, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let page = store.list_pages().unwrap().remove(0);
    let body = store.read_body(&page.rel_path).unwrap();
    assert!(
        body.contains("cas_legacy_scope: global"),
        "derived scope wrong:\n{body}"
    );
    assert!(body.contains("cas_legacy_db: global"));
}

// ── §11 — preconditions abort, never warn ───────────────────────────────

#[test]
fn a_compressed_row_aborts_the_run() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(&conn, "p-1", "context", Some("t"), "c");
        conn.execute(
            "UPDATE entries SET compressed = 1, raw_content = 'x' WHERE id = 'p-1'",
            [],
        )
        .unwrap();
    }
    let ledger = temp.path().join("ledger");
    let err = memory_migration::run(&config(vec![(DbLabel::Project, root)], &ledger, false))
        .unwrap_err()
        .to_string();
    assert!(err.contains("compressed"), "{err}");
}

#[test]
fn a_populated_graph_table_aborts_the_run() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(&conn, "p-1", "context", Some("t"), "c");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS code_memory_links (entry_id TEXT, symbol_id TEXT)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO code_memory_links VALUES ('p-1','sym')", [])
            .unwrap();
    }
    let ledger = temp.path().join("ledger");
    let err = memory_migration::run(&config(vec![(DbLabel::Project, root)], &ledger, false))
        .unwrap_err()
        .to_string();
    assert!(err.contains("code_memory_links"), "{err}");
}

#[test]
fn a_markdown_store_directory_aborts_the_run() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(&raw(&root), "p-1", "context", Some("t"), "c");
    std::fs::create_dir_all(root.join("entries")).unwrap();
    let ledger = temp.path().join("ledger");
    let err = memory_migration::run(&config(vec![(DbLabel::Project, root)], &ledger, false))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("MarkdownStore") || err.contains("entries/"),
        "{err}"
    );
}

#[test]
fn stranded_sync_queue_rows_block_apply_but_not_the_dry_run() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(&conn, "p-1", "context", Some("t"), "c");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sync_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                UNIQUE(entity_type, entity_id, team_id)
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_queue (entity_type, entity_id, operation, payload, created_at)
             VALUES ('entry','p-1','upsert','{\"id\":\"p-1\"}','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }
    let ledger = temp.path().join("ledger");

    // A dry run still reports — it is the tool that tells you the queue is dirty.
    let dry = memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        false,
    ))
    .unwrap();
    assert_eq!(dry.sync_queue_pending, 1);

    let err = memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("sync_queue"), "{err}");

    // §5.3 step 3 — invalidate, but only after the payloads are ledgered.
    let mut cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);
    cfg.invalidate_sync_queue = true;
    let out = memory_migration::run(&cfg).unwrap();
    assert_eq!(out.applied, 1);
    let ledgered = std::fs::read_to_string(ledger.join("sync-queue-invalidated.jsonl")).unwrap();
    assert!(
        ledgered.contains("p-1"),
        "payload was dropped without a ledger record"
    );
    let remaining: i64 = raw(&root)
        .query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE entity_type='entry'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);

    // The run that drained the queue must not then tell the operator to drain
    // it: `sync_queue_pending` is what is *still* outstanding, re-counted from
    // the database rather than assumed, and the drained rows are reported
    // separately so the ledger file can be found.
    assert_eq!(out.sync_queue_invalidated, 1);
    assert_eq!(out.sync_queue_pending, 0);
}

// ── §11 assert 6 — extraction is not capped at Store::list()'s LIMIT 10000 ──

#[test]
fn extraction_reads_past_the_store_list_row_cap() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..10_050 {
            conn.execute(
                "INSERT INTO entries (id, type, created, content) VALUES (?1,'learning',?2,?3)",
                params![
                    format!("p-{i:06}"),
                    "2026-01-01T00:00:00Z",
                    format!("row {i}")
                ],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();
    }
    let conn = source::open_read_only(&root.join("cas.db")).unwrap();
    let rows = source::extract_all(&conn, 500).unwrap();
    assert_eq!(
        rows.len(),
        10_050,
        "Store::list()'s LIMIT 10000 leaked into extraction"
    );
}

#[test]
fn the_source_connection_is_read_only() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(&raw(&root), "p-1", "context", Some("t"), "c");
    let conn = source::open_read_only(&root.join("cas.db")).unwrap();
    let err = conn
        .execute("DELETE FROM entries", [])
        .unwrap_err()
        .to_string();
    assert!(
        err.to_lowercase().contains("readonly"),
        "source DB is writable: {err}"
    );
}

#[test]
fn a_missing_source_database_is_an_error_not_a_fresh_empty_one() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("nope.db");
    assert!(source::open_read_only(&missing).is_err());
    assert!(!missing.exists(), "opening created the database");
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Everything about the destination that a re-apply must not change.
fn knowledge_snapshot(root: &Path) -> Vec<(String, String, String, bool, String, String, String)> {
    let store = SqliteKnowledgeStore::open(root).unwrap();
    let mut rows: Vec<_> = store
        .list_pages()
        .unwrap()
        .into_iter()
        .map(|p| {
            let body = store.read_body(&p.rel_path).unwrap();
            (
                p.id,
                p.rel_path.clone(),
                p.title,
                p.locked,
                p.created_at.to_rfc3339(),
                p.updated_at.to_rfc3339(),
                body,
            )
        })
        .collect();
    rows.sort();
    rows
}

// ── §6 — the reindex is an M3-owned step with a consistency check ───────

#[test]
fn reindex_rebuilds_the_page_index_and_verifies_retrievability() {
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    {
        let conn = raw(&root);
        insert_row(
            &conn,
            "p-ctx",
            "context",
            Some("Hooks"),
            "hooks live in cas-core",
        );
        insert_row(
            &conn,
            "p-pref",
            "preference",
            Some("Naming"),
            "prefer short names",
        );
    }
    let ledger = temp.path().join("ledger");
    let mut cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, true);
    cfg.reindex = true;
    let out = memory_migration::run(&cfg).unwrap();

    assert_eq!(out.index_reports.len(), 1);
    let report = &out.index_reports[0];
    assert_eq!(report.pages, 2);
    assert_eq!(report.reindexed, 2);
    assert!(report.missing_bodies.is_empty());
    assert!(
        report.unsearchable.is_empty(),
        "a written page was not retrievable"
    );
    report.check().unwrap();
    assert!(
        ledger.join("page-index.json").exists(),
        "index report must be ledgered"
    );

    // The reindex is genuinely functional, not just a counter.
    let store = SqliteKnowledgeStore::open(&root).unwrap();
    assert!(!store.search("hooks", 10).unwrap().is_empty());
}

#[test]
fn reindex_runs_standalone_without_apply() {
    // M5's cutover runbook defers execution of §6, not ownership: the step must
    // be invocable on its own after an earlier apply.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(
        &raw(&root),
        "p-ctx",
        "context",
        Some("Hooks"),
        "hooks live in cas-core",
    );
    let ledger = temp.path().join("ledger");

    memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let mut cfg = config(vec![(DbLabel::Project, root.clone())], &ledger, false);
    cfg.reindex = true;
    let out = memory_migration::run(&cfg).unwrap();
    assert_eq!(out.applied, 0, "a reindex-only run must not write pages");
    assert_eq!(out.index_reports[0].reindexed, 1);
}

#[test]
fn a_page_whose_body_vanished_fails_the_reindex() {
    // The consistency check must be able to fail, or it proves nothing.
    let temp = TempDir::new().unwrap();
    let root = legacy_root(&temp, "project");
    insert_row(
        &raw(&root),
        "p-ctx",
        "context",
        Some("Hooks"),
        "hooks live in cas-core",
    );
    let ledger = temp.path().join("ledger");
    memory_migration::run(&config(
        vec![(DbLabel::Project, root.clone())],
        &ledger,
        true,
    ))
    .unwrap();

    let store = SqliteKnowledgeStore::open(&root).unwrap();
    let page = store.list_pages().unwrap().remove(0);
    std::fs::remove_file(root.join("knowledge").join(&page.rel_path)).unwrap();

    let err = memory_migration::reindex::reindex_pages(&root)
        .unwrap()
        .check()
        .unwrap_err()
        .to_string();
    assert!(err.contains("no body file"), "{err}");
}
