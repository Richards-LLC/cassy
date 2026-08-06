//! MCP `knowledge` tool: the distilled-project-wiki page surface (cas-ee3d).
//!
//! These drive `CasCore` handlers directly, the same way the other tool suites
//! in this file do. The write path is the interesting one: a hand-written page
//! must come out `locked=true` so a later distillation pass cannot overwrite it.

use crate::support::*;
use cas_mcp::KnowledgeRequest;
use cas_store::{KnowledgeStore, SqliteKnowledgeStore};
use rmcp::handler::server::wrapper::Parameters;

/// A request with every optional field empty — tests fill in what they need.
fn req(action: &str) -> KnowledgeRequest {
    KnowledgeRequest {
        action: action.to_string(),
        query: None,
        id: None,
        rel_path: None,
        title: None,
        page_type: None,
        body: None,
        snippet: None,
        sources: None,
        include_body: None,
        limit: None,
    }
}

fn write_req(title: &str, page_type: &str, body: &str) -> KnowledgeRequest {
    KnowledgeRequest {
        title: Some(title.to_string()),
        page_type: Some(page_type.to_string()),
        body: Some(body.to_string()),
        ..req("write")
    }
}

/// Extract `[cas-knNNN]` from a write/read response.
fn page_id_of(text: &str) -> String {
    text.split('[')
        .nth(1)
        .and_then(|part| part.split(']').next())
        .expect("response should carry a page id in brackets")
        .to_string()
}

#[tokio::test]
async fn write_then_search_then_read_round_trips() {
    let (temp, core) = setup_cas();

    let created = extract_text(
        core.knowledge_write(Parameters(write_req(
            "Hook Dispatcher",
            "subsystem",
            "# Hook Dispatcher\n\nThe dispatcher fans SessionStart events out to registered handlers.\n",
        )))
        .await
        .expect("write should succeed"),
    );
    assert!(created.contains("Created"), "unexpected write output: {created}");
    // Canonical path is <type-slug>/<title-slug>.md — the merge key.
    assert!(
        created.contains("subsystem/hook-dispatcher.md"),
        "write should report the canonical path: {created}"
    );
    let id = page_id_of(&created);

    // search finds it by a word that only appears in the body.
    let found = extract_text(
        core.knowledge_search(Parameters(KnowledgeRequest {
            query: Some("dispatcher".to_string()),
            ..req("search")
        }))
        .await
        .expect("search should succeed"),
    );
    assert!(found.contains(&id), "search should surface the page: {found}");
    assert!(found.contains("Hook Dispatcher"), "search output: {found}");

    // read by id returns the body from disk.
    let read = extract_text(
        core.knowledge_read(Parameters(KnowledgeRequest {
            id: Some(id.clone()),
            ..req("read")
        }))
        .await
        .expect("read should succeed"),
    );
    assert!(
        read.contains("fans SessionStart events out to registered handlers"),
        "read should include the markdown body: {read}"
    );

    // read by rel_path resolves the same page.
    let by_path = extract_text(
        core.knowledge_read(Parameters(KnowledgeRequest {
            rel_path: Some("subsystem/hook-dispatcher.md".to_string()),
            ..req("read")
        }))
        .await
        .expect("read by path should succeed"),
    );
    assert!(by_path.contains(&id), "read by path output: {by_path}");

    // The body really is a file on disk, not a blob in SQLite.
    let body_file = temp
        .path()
        .join(".cas/knowledge/subsystem/hook-dispatcher.md");
    assert!(body_file.is_file(), "body should be a markdown file on disk");
}

#[tokio::test]
async fn a_hand_written_page_is_locked_so_distillation_cannot_overwrite_it() {
    let (temp, core) = setup_cas();

    let created = extract_text(
        core.knowledge_write(Parameters(write_req(
            "Build System",
            "architecture",
            "The workspace builds with cargo; zig backs the ghostty_vt_sys bindings.\n",
        )))
        .await
        .expect("write should succeed"),
    );
    assert!(
        created.contains("locked: true"),
        "write should report the lock: {created}"
    );

    // Assert against the store, not just the response text.
    let store = SqliteKnowledgeStore::open(&temp.path().join(".cas")).expect("store opens");
    let page = store
        .get_page_by_rel_path("architecture/build-system.md")
        .expect("lookup succeeds")
        .expect("page exists");
    assert!(page.locked, "a hand-written page must be locked");
    assert_eq!(page.sources, vec!["manual://mcp".to_string()]);
}

#[tokio::test]
async fn rewriting_a_locked_page_keeps_it_locked_and_keeps_its_id() {
    let (temp, core) = setup_cas();

    let first = extract_text(
        core.knowledge_write(Parameters(write_req(
            "Release Process",
            "workflow",
            "Cut a tag, then publish artifacts.\n",
        )))
        .await
        .expect("first write should succeed"),
    );
    let id = page_id_of(&first);

    let second = extract_text(
        core.knowledge_write(Parameters(write_req(
            "Release Process",
            "workflow",
            "Bump five crate versions, cut a tag, then publish artifacts.\n",
        )))
        .await
        .expect("rewrite should succeed"),
    );
    assert!(second.contains("Updated"), "rewrite output: {second}");
    assert_eq!(page_id_of(&second), id, "rewrite must not fork a new page");

    let store = SqliteKnowledgeStore::open(&temp.path().join(".cas")).expect("store opens");
    let page = store
        .get_page_by_rel_path("workflow/release-process.md")
        .expect("lookup succeeds")
        .expect("page exists");
    assert!(page.locked, "the page must still be locked after a rewrite");

    let body = store.read_body(&page.rel_path).expect("body readable");
    assert!(
        body.contains("Bump five crate versions"),
        "the rewrite should have landed: {body}"
    );

    let pages = store.list_pages().expect("list succeeds");
    assert_eq!(pages.len(), 1, "rewrite must not duplicate the page");
}

/// The write path unlocks before it ingests, so a failed ingest must put the
/// lock back. Otherwise one failed write silently strips a user's sovereignty
/// bit and the next distillation pass is free to overwrite the page.
///
/// The failure is injected by replacing the page's parent directory with a
/// symlink pointing outside the knowledge dir. `commit_ingest` resolves the
/// body's parent and refuses to write through anything that escapes
/// `.cas/knowledge/` (`ensure_within_knowledge_dir`), so the write fails
/// *after* the unlock — exactly the window the restore logic exists to cover.
///
/// Chosen over the obvious "make the body path unwritable" injection because
/// this one does not depend on file permissions, which a root test runner
/// ignores.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_write_restores_the_lock_it_took() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    core.knowledge_write(Parameters(write_req(
        "Lease Renewal",
        "workflow",
        "Heartbeat renews the lease every thirty seconds.\n",
    )))
    .await
    .expect("first write should succeed");

    let store = SqliteKnowledgeStore::open(&cas_dir).expect("store opens");
    let page = store
        .get_page_by_rel_path("workflow/lease-renewal.md")
        .expect("lookup succeeds")
        .expect("page exists");
    assert!(page.locked, "precondition: the page starts locked");

    // Point the page's directory outside the knowledge dir; the store's
    // containment check then refuses the write.
    let body_path = store.body_path(&page.rel_path).expect("body path");
    let page_dir = body_path.parent().expect("page dir").to_path_buf();
    let escape = temp.path().join("outside-the-knowledge-dir");
    std::fs::create_dir_all(&escape).expect("create escape dir");
    std::fs::remove_dir_all(&page_dir).expect("remove page dir");
    std::os::unix::fs::symlink(&escape, &page_dir).expect("symlink page dir out of the store");

    let failed = core
        .knowledge_write(Parameters(write_req(
            "Lease Renewal",
            "workflow",
            "Heartbeat renews the lease every ten seconds.\n",
        )))
        .await;
    assert!(failed.is_err(), "the write must fail, not silently no-op");

    // The whole point: the lock survived a failed write.
    let after = SqliteKnowledgeStore::open(&cas_dir)
        .expect("store reopens")
        .get_page_by_rel_path("workflow/lease-renewal.md")
        .expect("lookup succeeds")
        .expect("page still exists");
    assert!(
        after.locked,
        "a failed write must leave the page exactly as locked as it found it"
    );
}

#[tokio::test]
async fn list_and_status_report_an_empty_store_without_erroring() {
    let (_temp, core) = setup_cas();

    let list = extract_text(
        core.knowledge_list(Parameters(req("list")))
            .await
            .expect("list should succeed on an empty store"),
    );
    assert!(list.contains("No distilled pages yet"), "list output: {list}");

    let status = extract_text(
        core.knowledge_status(Parameters(req("status")))
            .await
            .expect("status should succeed on an empty store"),
    );
    assert!(status.contains("pages:   0"), "status output: {status}");

    let search = extract_text(
        core.knowledge_search(Parameters(KnowledgeRequest {
            query: Some("anything".to_string()),
            ..req("search")
        }))
        .await
        .expect("search should succeed on an empty store"),
    );
    assert!(search.contains("No distilled pages match"), "search output: {search}");
}

#[tokio::test]
async fn write_rejects_a_missing_body_and_a_missing_title() {
    let (_temp, core) = setup_cas();

    let no_body = core
        .knowledge_write(Parameters(KnowledgeRequest {
            title: Some("Empty".to_string()),
            ..req("write")
        }))
        .await;
    assert!(no_body.is_err(), "a write with no body must be rejected");

    let no_title = core
        .knowledge_write(Parameters(KnowledgeRequest {
            body: Some("orphan prose".to_string()),
            ..req("write")
        }))
        .await;
    assert!(no_title.is_err(), "a write with no title or path must be rejected");
}

#[tokio::test]
async fn read_of_an_unknown_page_is_an_error_not_an_empty_success() {
    let (_temp, core) = setup_cas();

    assert!(
        core.knowledge_read(Parameters(KnowledgeRequest {
            id: Some("cas-kn999".to_string()),
            ..req("read")
        }))
        .await
        .is_err(),
        "reading a nonexistent id must error"
    );

    assert!(
        core.knowledge_read(Parameters(req("read"))).await.is_err(),
        "read with neither id nor rel_path must error"
    );
}

#[tokio::test]
async fn a_snippet_is_derived_from_the_body_when_the_caller_omits_one() {
    let (temp, core) = setup_cas();

    core.knowledge_write(Parameters(write_req(
        "Task Leases",
        "subsystem",
        "---\ntitle: Task Leases\n---\n\n# Task Leases\n\nA lease is a time-boxed claim on a task, renewed by heartbeat.\n",
    )))
    .await
    .expect("write should succeed");

    let store = SqliteKnowledgeStore::open(&temp.path().join(".cas")).expect("store opens");
    let page = store
        .get_page_by_rel_path("subsystem/task-leases.md")
        .expect("lookup succeeds")
        .expect("page exists");
    assert_eq!(
        page.snippet,
        "A lease is a time-boxed claim on a task, renewed by heartbeat.",
        "the snippet should skip frontmatter and headings"
    );
}
