//! Consume either an immediate merge result or its durable continuation.
use std::path::Path;

use rmcp::model::CallToolResult;
use serde_json::Value;

pub async fn completed(root: &Path, task: &str, answer: CallToolResult) -> (CallToolResult, Option<Value>) {
    let Some(data) = answer.structured_content.as_ref().filter(|data| data["receipt_path"].is_string()) else {
        return (answer, None);
    };
    assert!(matches!((data["mutation_outcome"].as_str(), data["state"].as_str()),
        (Some("COMMITTED"), Some("merged")) | (Some("IN_PROGRESS"), Some("preparing"))));
    let path = data["receipt_path"].as_str().unwrap();
    let id = data["receipt_id"].as_str().unwrap();
    let store = cas::store::open_task_store(root).unwrap();
    let notices = cas::store::open_prompt_queue_store(root).unwrap();
    let saved = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            // Keep the fixture alive through both durable outcome channels.
            if saved["state"] == "completed" && store.get(task).unwrap().notes.contains(id)
                && notices.peek_all(1000).unwrap().iter().any(|notice| notice.prompt.contains(id)) {
                break saved;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("merge continuation must persist its result and task citation");
    assert_eq!(saved["receipt_id"], id);
    let result = serde_json::from_value(saved["result"]["ok"].clone())
        .unwrap_or_else(|error| panic!("merge continuation did not return a tool result: {error}; {saved}"));
    (result, Some(saved))
}
