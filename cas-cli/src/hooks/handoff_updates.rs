//! Bounded Violet context newer than the handoff being resumed.

use cas_types::Entry;
use serde_json::Value;

fn enrich(context: &str, _handoff: &Entry, _read: impl FnMut(Value) -> Result<Value, String>) -> String {
    context.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_core::hooks::{ContextStores, DefaultHooksConfig, HookInput, build_context_with_stores};
    use cas_store::{SqliteStore, Store};
    use serde_json::json;

    #[test]
    fn session_start_lists_posts_newer_than_named_handoff_and_omits_quiet_section() {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(temp.path()).unwrap();
        store.init().unwrap();
        let handoff = Entry {
            id: "handoff-new".into(),
            content: "Resume https://example.slack.com/archives/C123/p1793712000000000 in #project-chat".into(),
            created: "2026-11-03T13:20:00Z".parse().unwrap(),
            tags: vec!["handoff".into(), "role:supervisor".into()],
            ..Default::default()
        };
        let mut older = handoff.clone();
        older.id = "handoff-old".into();
        older.created -= chrono::Duration::hours(1);
        older.content = "Stale #wrong-channel".into();
        store.add(&older).unwrap();
        store.add(&handoff).unwrap();
        let input = HookInput {
            hook_event_name: "SessionStart".into(),
            agent_role: Some("supervisor".into()),
            ..Default::default()
        };
        let context = build_context_with_stores(
            &input,
            &ContextStores { project_store: Some(&store), ..ContextStores::empty() },
            &DefaultHooksConfig::new(), 10, None, "mcp__cs__",
        ).unwrap().0;
        let message = |id, time, text| json!({
            "message_id":id, "created_at":time, "text":text,
            "author":{"id":"U123","is_bot":false},
            "permalink":format!("https://example.slack.com/archives/C123/p{id}")
        });
        let posts = vec![
            message("1", "2026-11-03T13:20:00Z", "At the handoff boundary"),
            message("2", "2026-11-03T13:35:00Z", "Check the channel too"),
            message("3", "2026-11-03T13:49:00Z", "The plan changed"),
        ];
        let updated = enrich(&context, &handoff, |args| {
            assert_eq!(args["since"], "2026-11-03T13:20:00Z");
            assert_ne!(args["channel"], "wrong-channel");
            assert!(args.get("thread_id").is_none(), "thread references must include sibling channel posts");
            Ok(json!({"ok":true,"complete":true,"messages":posts}))
        });
        let saved = updated.find("## 🔁 Current Handoff").unwrap();
        for text in ["Check the channel too", "The plan changed"] {
            assert!(updated.find(text).is_some_and(|pos| pos < saved), "{updated}");
        }
        assert!(!updated.contains("At the handoff boundary"));
        let quiet = enrich(&context, &handoff, |_| Ok(json!({"ok":true,"complete":true,"messages":[]})));
        assert_eq!(quiet, context, "quiet channels add no section");
    }
}
