//! cas-c7c2 — `memory recent` ordering contract.

use crate::support::*;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use rusqlite::Connection;

fn remember_request(content: &str, title: &str) -> RememberRequest {
    RememberRequest {
        scope: "project".to_string(),
        content: content.to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: Some(title.to_string()),
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        personal: None,
    }
}

#[tokio::test]
async fn recent_declares_and_applies_stable_id_tie_break() {
    let (temp, service) = setup_cas();

    let first = service
        .cas_remember(Parameters(remember_request(
            "recent ordering first entry",
            "first",
        )))
        .await
        .expect("first entry should be stored");
    let first_id = extract_entry_id(&extract_text(first))
        .expect("first entry id")
        .to_string();

    let second = service
        .cas_remember(Parameters(remember_request(
            "recent ordering second entry",
            "second",
        )))
        .await
        .expect("second entry should be stored");
    let second_id = extract_entry_id(&extract_text(second))
        .expect("second entry id")
        .to_string();
    assert!(second_id > first_id, "fixture IDs must be ascending");

    let conn = Connection::open(temp.path().join(".cas/cas.db"))
        .expect("test store database should be reachable");
    conn.execute(
        "UPDATE entries SET created = ?1, updated_at = ?1 WHERE id IN (?2, ?3)",
        rusqlite::params!["2026-08-09T12:00:00+00:00", first_id, second_id],
    )
    .expect("fixture timestamps should tie");

    let result = service
        .cas_recent(Parameters(RecentRequest { n: 2 }))
        .await
        .expect("recent should succeed");
    let text = extract_text(result);

    assert!(
        text.contains("ordered_by: recent_at desc, id desc"),
        "recent response must state its ordering contract: {text}"
    );
    assert!(
        text.find(&format!("[{second_id}]")).unwrap()
            < text.find(&format!("[{first_id}]")).unwrap(),
        "equal recent timestamps must be ordered by descending id: {text}"
    );
}
