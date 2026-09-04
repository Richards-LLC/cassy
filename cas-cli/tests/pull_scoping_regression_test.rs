//! Regression guard for cas-2eb3 / cas-ed15.
//!
//! Before cas-ed15, `cas-cli/src/cli/cloud.rs::execute_pull` had its own inline
//! `ureq::get(format!("{}/api/sync/pull", ...))` URL builder that never appended
//! `project_id=`. The `cas cloud pull` CLI command therefore issued unscoped pulls
//! and imported cross-project rows into the local DB — the cas-2eb3 contamination
//! vector.
//!
//! These tests lock in two invariants on the `cas` crate:
//!
//! 1. **Source-level**: there is exactly one production URL builder for
//!    `/api/sync/pull`, and it lives in the scoped syncer
//!    (`cas-cli/src/cloud/syncer/pull.rs`). Any future regression that
//!    re-introduces a second inline builder will fail this test.
//!
//! 2. **Wire-level**: when `CloudSyncer::pull` is invoked, the URL on the wire
//!    includes a `project_id=` query parameter. This is the runtime contract
//!    the CLI now depends on.

use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE_PROJECT_ID: &str = "p";

/// Roots that contain shipped source code. Tests, fixtures, and benches are
/// excluded — they are allowed to construct pull URLs freely.
fn production_source_root() -> PathBuf {
    cas::test_paths::crate_root().join("src")
}

/// Recursively collect every `.rs` file under `dir`.
fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Tests/benches/fixtures live elsewhere; nothing to skip under src/.
            collect_rust_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// Strip line comments and block comments so we never match a literal that
/// only appears in a doc comment. This is a coarse pass — it does not
/// implement the full Rust tokenizer, but it correctly handles `// ...`
/// to end-of-line and `/* ... */` (non-nested). Inline strings are not
/// stripped because the bug is *about* a string literal.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        // Line comment
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            // Skip to end of line; preserve the newline so line counts roughly align.
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = i.saturating_add(2).min(bytes.len());
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Blank inline `#[cfg(test)] mod ... { ... }` spans while preserving line
/// breaks. The guard scans files under `src/`, where an inline test module is
/// still present on disk but is not shipped production code.
fn strip_cfg_test_modules(src: &str) -> String {
    let mut out = src.as_bytes().to_vec();
    let mut search_from = 0;

    while let Some(offset) = src[search_from..].find("#[cfg(test)]") {
        let attribute_start = search_from + offset;
        let after_attribute = attribute_start + "#[cfg(test)]".len();

        let Some(open_brace) = cfg_test_module_open_brace(src, after_attribute) else {
            search_from = after_attribute;
            continue;
        };
        let Some(close_brace) = matching_brace(src, open_brace) else {
            // Leave malformed source unchanged; rustc will report the actual
            // syntax error, while this guard should remain conservative.
            search_from = open_brace + 1;
            continue;
        };

        for byte in &mut out[attribute_start..=close_brace] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        search_from = close_brace + 1;
    }

    String::from_utf8(out).expect("source bytes remain valid UTF-8")
}

/// Return the opening brace for the module immediately gated by `#[cfg(test)]`.
fn cfg_test_module_open_brace(src: &str, after_attribute: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = after_attribute;

    // `pub(crate) mod tests { ... }` is valid too, so seek the `mod` keyword
    // before either the item's body or terminator.
    while i < bytes.len() {
        if bytes[i] == b'{' || bytes[i] == b';' {
            return None;
        }
        if bytes[i..].starts_with(b"mod")
            && (i == 0 || !is_identifier_byte(bytes[i - 1]))
            && !is_identifier_byte(*bytes.get(i + 3).unwrap_or(&b' '))
        {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j] != b'{' && bytes[j] != b';' {
                j += 1;
            }
            return (bytes.get(j) == Some(&b'{')).then_some(j);
        }
        i += 1;
    }
    None
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Find the closing brace for an inline module, ignoring braces in ordinary
/// quoted literals. Comments are already stripped before this helper runs.
fn matching_brace(src: &str, open_brace: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open_brace;

    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == quote {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn pull_url_reference_lines(src: &str) -> Vec<usize> {
    let without_comments = strip_comments(src);
    let production_only = strip_cfg_test_modules(&without_comments);
    production_only
        .lines()
        .enumerate()
        .filter_map(|(i, line)| line.contains("/api/sync/pull").then_some(i + 1))
        .collect()
}

/// Scan one source root for pull URL references, after removing non-production
/// inline test modules.
fn pull_url_hits(root: &Path) -> Vec<(PathBuf, Vec<usize>)> {
    let mut files = Vec::new();
    collect_rust_files(root, &mut files);

    files
        .into_iter()
        .filter_map(|path| {
            let src = fs::read_to_string(&path).ok()?;
            let line_numbers = pull_url_reference_lines(&src);
            (!line_numbers.is_empty()).then_some((path, line_numbers))
        })
        .collect()
}

#[test]
fn pull_url_scan_ignores_cfg_test_module_fixture() {
    let fixture = cas::test_paths::crate_root()
        .join("tests")
        .join("fixtures")
        .join("pull_scoping")
        .join("cfg_test_module_only.rs");
    let src = fs::read_to_string(&fixture).expect("read cfg(test) pull URL fixture");

    assert!(
        pull_url_reference_lines(&src).is_empty(),
        "references inside #[cfg(test)] modules must not be reported as production: {}",
        fixture.display(),
    );
}

#[test]
fn pull_url_scan_still_detects_production_reference() {
    let src = "const PULL_PATH: &str = \"/api/sync/pull\";\n\
               #[cfg(test)]\n\
               mod tests { const MATCHER: &str = \"/api/sync/pull\"; }\n";

    assert_eq!(pull_url_reference_lines(src), vec![1]);
}

#[test]
fn only_one_production_pull_url_builder_exists() {
    let root = production_source_root();

    // Files that legitimately contain `/api/sync/pull` as code (not comments).
    let hits = pull_url_hits(&root);

    // The single allowed builder lives in the scoped syncer.
    let expected = root.join("cloud").join("syncer").join("pull.rs");

    let unexpected: Vec<_> = hits.iter().filter(|(p, _)| p != &expected).collect();

    assert!(
        unexpected.is_empty(),
        "Found unexpected production `/api/sync/pull` reference(s) outside the scoped syncer.\n\
         This is a cas-2eb3 / cas-ed15 regression: every code path that issues a\n\
         `/api/sync/pull` request MUST go through `CloudSyncer::pull`, which appends\n\
         `?project_id=`. A second builder will issue unscoped pulls and re-introduce\n\
         the cross-project contamination this guard exists to prevent.\n\
         Offenders:\n{}\n\n\
         Fix: route the new caller through `crate::cloud::CloudSyncer::pull` (see\n\
         `cas-cli/src/cli/cloud.rs::execute_pull` for the canonical pattern), or\n\
         extend the syncer surface if a new entity kind is needed.",
        unexpected
            .iter()
            .map(|(p, ls)| format!(
                "  - {} (lines: {})",
                p.display(),
                ls.iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    // And the scoped builder must still be present — if it was renamed or
    // moved we want this test to fail loudly rather than silently pass.
    assert!(
        hits.iter().any(|(p, _)| p == &expected),
        "Expected the scoped pull URL builder at {} to remain. If you moved it, \
         update this regression test to point at the new location.",
        expected.display(),
    );
}

#[test]
fn scoped_pull_builder_appends_project_id() {
    // Belt-and-suspenders source-level assertion: the one allowed builder
    // must, in the same file, also append `project_id=`. This catches a
    // regression where someone deletes the project_id line in pull.rs
    // without touching the URL format string.
    let pull_rs =
        production_source_root().join("cloud").join("syncer").join("pull.rs");
    let src = fs::read_to_string(&pull_rs).expect("read syncer/pull.rs");
    assert!(
        src.contains("project_id="),
        "{} must construct a `project_id=` query parameter — that's the scoping \
         contract every `/api/sync/pull` request depends on.",
        pull_rs.display(),
    );
    assert!(
        src.contains("resolve_canonical_id_for_sync"),
        "{} must resolve the project scope from the sync root, not process cwd.",
        pull_rs.display(),
    );
    // cas-0be9: the builder must FAIL CLOSED. Dropping `project_id=` when the
    // scope is unresolvable (an `if let Some(..)` around the push) issues an
    // unscoped pull, which is the contamination this guard exists to prevent.
    assert!(
        src.contains("Cannot pull: not inside a CAS project directory"),
        "{} must abort the pull when the project scope cannot be resolved, \
         rather than silently omitting `project_id=`.",
        pull_rs.display(),
    );
}

#[tokio::test]
async fn cloud_syncer_pull_request_carries_project_id_on_the_wire() {
    use std::sync::Arc;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Spin up a mock cloud, configure a syncer pointing at it, and assert
    // that the URL on the wire carries `project_id=<resolved>`. This locks
    // in the runtime contract `execute_pull` now depends on.
    let server = MockServer::start().await;

    // The syncer resolves identity from its queue root. This fixture pins that
    // root to `p`, independently of the integration-test process cwd.
    let expected_project_id = FIXTURE_PROJECT_ID.to_string();

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", expected_project_id.as_str()))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [],
                "tasks": [],
                "rules": [],
                "skills": [],
                "pulled_at": chrono::Utc::now().to_rfc3339(),
            })),
        )
        .expect(1) // exactly one matching request
        .mount(&server)
        .await;

    let cloud_config = cas::cloud::CloudConfig {
        endpoint: server.uri(),
        token: Some("test-token".to_string()),
        ..Default::default()
    };

    // Spin up an in-memory store set. The syncer needs trait objects but
    // we never actually need to upsert anything — the response body is empty.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("mkdir .cas");
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"p\"\n",
    )
    .expect("pin fixture project identity");

    let store = cas::store::open_store(&cas_root).expect("open store");
    let task_store = cas::store::open_task_store(&cas_root).expect("open task store");
    let rule_store = cas::store::open_rule_store(&cas_root).expect("open rule store");
    let skill_store = cas::store::open_skill_store(&cas_root).expect("open skill store");
    let spec_store = cas::store::open_spec_store(&cas_root).expect("open spec store");
    let event_store = cas::store::open_event_store(&cas_root).expect("open event store");
    let prompt_store = cas::store::open_prompt_store(&cas_root).expect("open prompt store");
    let file_change_store =
        cas::store::open_file_change_store(&cas_root).expect("open file change store");
    let commit_link_store =
        cas::store::open_commit_link_store(&cas_root).expect("open commit link store");

    let queue = cas::cloud::SyncQueue::open(&cas_root).expect("open queue");
    queue.init().expect("init queue");

    let syncer = cas::cloud::CloudSyncer::new(
        Arc::new(queue),
        cloud_config,
        cas::cloud::CloudSyncerConfig::default(),
    );

    let result = syncer
        .pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )
        .expect("pull should succeed against the mock");

    assert!(
        result.errors.is_empty(),
        "Pull should not produce errors against the matching mock; got: {:?}",
        result.errors,
    );
    // wiremock's `.expect(1)` enforces that exactly one matching request fired.
    // If `project_id=` is missing or wrong, no mock matches → 404 → CasError.
}

/// cas-de89 regression: task ownership is the SQLite database selected by the
/// caller, not a persisted `project_id` column. A scoped cloud pull must only
/// admit rows for the current project into that database, and neither direct
/// project writes nor direct global writes may bleed into the other store.
#[tokio::test]
async fn task_pull_and_direct_writes_remain_isolated_by_store() {
    use std::sync::Arc;

    use cas::types::Task;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let expected_project_id = FIXTURE_PROJECT_ID.to_string();

    let task_json = |id: &str, project_id: Option<&str>| {
        let mut value = serde_json::to_value(Task::new(id.to_string(), format!("task {id}")))
            .expect("serialize task fixture");
        if let Some(project_id) = project_id {
            value.as_object_mut().expect("task fixture object").insert(
                "project_canonical_id".to_string(),
                serde_json::Value::String(project_id.to_string()),
            );
        }
        value
    };

    let matching = task_json("cas-project-pull", Some(&expected_project_id));
    let foreign = task_json("cas-foreign-pull", Some("unrelated/product"));
    let mut replicated = task_json("cas-replicated-pull", Some(&expected_project_id));
    replicated["origin_project"] = serde_json::json!("unrelated/product");
    let unscoped = task_json("cas-unscoped-pull", None);

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", expected_project_id.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [],
            "tasks": [matching, foreign, replicated, unscoped],
            "rules": [],
            "skills": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().join("project").join(".cas");
    let global_root = tmp.path().join("global").join(".cas");
    std::fs::create_dir_all(&project_root).expect("mkdir project .cas");
    std::fs::create_dir_all(&global_root).expect("mkdir global .cas");
    std::fs::write(
        project_root.join("config.toml"),
        "[project]\ncanonical_id = \"p\"\n",
    )
    .expect("pin project fixture identity");

    let project_tasks =
        cas::store::open_task_store_local(&project_root).expect("open isolated project task store");
    let global_tasks =
        cas::store::open_task_store_local(&global_root).expect("open isolated global task store");

    let global_only = Task::new("cas-global-only".to_string(), "global only".to_string());
    global_tasks.add(&global_only).expect("seed global store");
    let project_only = Task::new("cas-project-only".to_string(), "project only".to_string());
    project_tasks
        .add(&project_only)
        .expect("seed project store");

    let store = cas::store::open_store_local(&project_root).expect("open entry store");
    let rule_store = cas::store::open_rule_store_local(&project_root).expect("open rule store");
    let skill_store = cas::store::open_skill_store_local(&project_root).expect("open skill store");
    let spec_store = cas::store::open_spec_store(&project_root).expect("open spec store");
    let event_store = cas::store::open_event_store(&project_root).expect("open event store");
    let prompt_store = cas::store::open_prompt_store(&project_root).expect("open prompt store");
    let file_change_store =
        cas::store::open_file_change_store(&project_root).expect("open file change store");
    let commit_link_store =
        cas::store::open_commit_link_store(&project_root).expect("open commit link store");
    let queue = Arc::new(
        cas::cloud::SyncQueue::open(&project_root).expect("open queue"),
    );
    queue.init().expect("init queue");

    let syncer = cas::cloud::CloudSyncer::new(
        Arc::clone(&queue),
        cas::cloud::CloudConfig {
            endpoint: server.uri(),
            token: Some("synthetic-test-token".to_string()),
            ..Default::default()
        },
        cas::cloud::CloudSyncerConfig::default(),
    );
    let result = syncer
        .pull(
            store.as_ref(),
            project_tasks.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )
        .expect("scoped task pull");

    assert_eq!(
        result.pulled_tasks, 1,
        "only the matching task may be imported"
    );
    assert!(project_tasks.get("cas-project-pull").is_ok());
    assert!(project_tasks.get("cas-foreign-pull").is_err());
    assert!(project_tasks.get("cas-replicated-pull").is_err());
    assert!(project_tasks.get("cas-unscoped-pull").is_err());
    assert!(project_tasks.get("cas-global-only").is_err());

    assert!(global_tasks.get("cas-global-only").is_ok());
    assert!(global_tasks.get("cas-project-only").is_err());
    assert!(global_tasks.get("cas-project-pull").is_err());

    let conflicts = queue.list_conflicts(20).expect("pull conflict journal");
    assert!(
        conflicts.iter().any(|conflict| {
            conflict.entity_id == "cas-replicated-pull"
                && conflict.strategy == "pull_foreign_origin"
                && conflict.discarded_row_json.contains("unrelated/product")
        }),
        "explicit foreign-origin personal rows must be durably parked: {conflicts:?}"
    );
}

/// cas-bba4 regression: the 5 entity kinds re-added to `CloudSyncer::pull`
/// (specs / events / prompts / file_changes / commit_links) must honor the
/// same client-side `entity_matches_project` filter as entries/tasks/rules/
/// skills. This test seeds the mock with one matching + one foreign payload
/// per kind and asserts that pulled_* counts == 1 (matching only).
///
/// `specs` is omitted from the mock — cloud doesn't return that key yet
/// (see docs/requests/FEATURE-cloud-sync-pull-return-specs.md). When the
/// cloud ships specs, extend this test to mirror the other 4 kinds.
#[tokio::test]
async fn new_entity_kinds_skip_foreign_project_rows() {
    use std::sync::Arc;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let expected_project_id = FIXTURE_PROJECT_ID.to_string();

    let now = chrono::Utc::now().to_rfc3339();

    // One matching + one foreign payload per kind. `entity_matches_project`
    // skips rows whose `project_canonical_id` doesn't equal the resolved
    // project; matching rows are imported.
    let response_body = serde_json::json!({
        "entries": [],
        "tasks": [],
        "rules": [],
        "skills": [],
        "events": [
            {
                "id": 1,
                "event_type": "task_created",
                "entity_type": "task",
                "entity_id": "cas-aaaa",
                "summary": "matching",
                "metadata": null,
                "created_at": now,
                "session_id": null,
                "project_canonical_id": expected_project_id,
            },
            {
                "id": 2,
                "event_type": "task_created",
                "entity_type": "task",
                "entity_id": "cas-bbbb",
                "summary": "foreign",
                "metadata": null,
                "created_at": now,
                "session_id": null,
                "project_canonical_id": "some-other-project",
            },
        ],
        "prompts": [
            {
                "id": "prompt-matching",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "content": "matching prompt",
                "content_hash": "deadbeef",
                "timestamp": now,
                "project_canonical_id": expected_project_id,
            },
            {
                "id": "prompt-foreign",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "content": "foreign prompt",
                "content_hash": "cafebabe",
                "timestamp": now,
                "project_canonical_id": "some-other-project",
            },
        ],
        "file_changes": [
            {
                "id": "fc-matching",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "prompt_id": null,
                "repository": "repo",
                "file_path": "foo.rs",
                "file_id": null,
                "change_type": "modified",
                "tool_name": "Edit",
                "old_content_hash": null,
                "new_content_hash": "abc",
                "commit_hash": null,
                "committed_at": null,
                "created_at": now,
                "scope": "project",
                "project_canonical_id": expected_project_id,
            },
            {
                "id": "fc-foreign",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "prompt_id": null,
                "repository": "repo",
                "file_path": "bar.rs",
                "file_id": null,
                "change_type": "modified",
                "tool_name": "Edit",
                "old_content_hash": null,
                "new_content_hash": "def",
                "commit_hash": null,
                "committed_at": null,
                "created_at": now,
                "scope": "project",
                "project_canonical_id": "some-other-project",
            },
        ],
        "commit_links": [
            {
                "commit_hash": "1111111111111111111111111111111111111111",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "branch": "main",
                "message": "matching commit",
                "files_changed": [],
                "prompt_ids": [],
                "committed_at": now,
                "author": "tester",
                "scope": "project",
                "project_canonical_id": expected_project_id,
            },
            {
                "commit_hash": "2222222222222222222222222222222222222222",
                "session_id": "session-A",
                "agent_id": "agent-A",
                "branch": "main",
                "message": "foreign commit",
                "files_changed": [],
                "prompt_ids": [],
                "committed_at": now,
                "author": "tester",
                "scope": "project",
                "project_canonical_id": "some-other-project",
            },
        ],
        "pulled_at": now,
    });

    Mock::given(method("GET"))
        .and(path("/api/sync/pull"))
        .and(query_param("project_id", expected_project_id.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
        .expect(1)
        .mount(&server)
        .await;

    let cloud_config = cas::cloud::CloudConfig {
        endpoint: server.uri(),
        token: Some("test-token".to_string()),
        ..Default::default()
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_root = tmp.path().join(".cas");
    std::fs::create_dir_all(&cas_root).expect("mkdir .cas");
    std::fs::write(
        cas_root.join("config.toml"),
        "[project]\ncanonical_id = \"p\"\n",
    )
    .expect("pin fixture project identity");

    let store = cas::store::open_store(&cas_root).expect("open store");
    let task_store = cas::store::open_task_store(&cas_root).expect("open task store");
    let rule_store = cas::store::open_rule_store(&cas_root).expect("open rule store");
    let skill_store = cas::store::open_skill_store(&cas_root).expect("open skill store");
    let spec_store = cas::store::open_spec_store(&cas_root).expect("open spec store");
    let event_store = cas::store::open_event_store(&cas_root).expect("open event store");
    let prompt_store = cas::store::open_prompt_store(&cas_root).expect("open prompt store");
    let file_change_store =
        cas::store::open_file_change_store(&cas_root).expect("open file change store");
    let commit_link_store =
        cas::store::open_commit_link_store(&cas_root).expect("open commit link store");

    let queue = cas::cloud::SyncQueue::open(&cas_root).expect("open queue");
    queue.init().expect("init queue");

    let syncer = cas::cloud::CloudSyncer::new(
        Arc::new(queue),
        cloud_config,
        cas::cloud::CloudSyncerConfig::default(),
    );

    let result = syncer
        .pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )
        .expect("pull should succeed against the matching mock");

    assert_eq!(
        result.pulled_events, 1,
        "expected exactly one matching event imported, got {} (errors: {:?})",
        result.pulled_events, result.errors,
    );
    assert_eq!(
        result.pulled_prompts, 1,
        "expected exactly one matching prompt imported, got {} (errors: {:?})",
        result.pulled_prompts, result.errors,
    );
    assert_eq!(
        result.pulled_file_changes, 1,
        "expected exactly one matching file_change imported, got {} (errors: {:?})",
        result.pulled_file_changes, result.errors,
    );
    assert_eq!(
        result.pulled_commit_links, 1,
        "expected exactly one matching commit_link imported, got {} (errors: {:?})",
        result.pulled_commit_links, result.errors,
    );
    // `specs` is intentionally omitted from the mock body; verify the
    // syncer defends against a missing key (forward-compat for the cloud
    // pull-endpoint extension).
    assert_eq!(
        result.pulled_specs, 0,
        "expected zero specs when the response omits the key, got {}",
        result.pulled_specs,
    );
}
