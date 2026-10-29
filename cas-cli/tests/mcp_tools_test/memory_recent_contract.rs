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
        expected_updated_at: None,
        personal: None,
    }
}

#[tokio::test]
async fn recent_declares_and_applies_stable_id_tie_break() {
    let mut test_env = TestEnvGuard::temp_home();
    let (temp, service) = setup_cas(&mut test_env);

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
    let conn = Connection::open(temp.path().join(".cas/cas.db")).unwrap();
    use rand::{Rng, SeedableRng};
    for seed in 0..128 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let first_id = format!("mem-{:032x}", rng.random::<u128>());
        let second_id = format!("mem-{:032x}", rng.random::<u128>());
        for (title, id) in [("first", &first_id), ("second", &second_id)] {
            assert_eq!(
                conn.execute(
                    "UPDATE entries SET id = ?1, created = ?2, updated_at = ?2 WHERE title = ?3",
                    rusqlite::params![id, "2026-08-09T12:00:00+00:00", title],
                )
                .unwrap(),
                1
            );
        }
        // RED control: historical assumption; remove for the GREEN seed sweep.
        assert!(
            second_id > first_id,
            "seed {seed}: fixture IDs must be ascending: {first_id}, {second_id}"
        );
        assert_ne!(first_id, second_id);
        let mut expected = [&first_id, &second_id];
        expected.sort_unstable();
        let [lower, higher] = expected;
        let text = extract_text(
            service
                .cas_recent(Parameters(RecentRequest { n: 2 }))
                .await
                .unwrap(),
        );
        assert!(
            text.contains("ordered_by: recent_at desc, id desc"),
            "seed {seed}: {text}"
        );
        assert!(
            text.find(&format!("[{higher}]")).unwrap() < text.find(&format!("[{lower}]")).unwrap(),
            "seed {seed}: {text}"
        );
    }
}
