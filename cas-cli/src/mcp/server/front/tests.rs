use super::*;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::{Mutex, Notify};
use std::time::Duration;

async fn send(output: &mut (impl AsyncWrite + Unpin), message: Value) {
    let mut bytes = serde_json::to_vec(&message).unwrap();
    bytes.push(b'\n');
    output.write_all(&bytes).await.unwrap();
    output.flush().await.unwrap();
}

async fn receive(input: &mut BufReader<DuplexStream>) -> Value {
    let mut line = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), input.read_line(&mut line))
        .await.expect("front must answer without closing or wedging the client").unwrap();
    assert!(n > 0, "front closed the initialized client connection");
    serde_json::from_str(&line).unwrap()
}

struct Fixture {
    input: BufReader<DuplexStream>,
    output: DuplexStream,
    updates: mpsc::Sender<Backend>,
    events: mpsc::Receiver<Event>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    started: Arc<Notify>,
    finish: Arc<Notify>,
    front: tokio::task::JoinHandle<Result<()>>,
}

impl Fixture {
    async fn new() -> Self {
        let (client_write, front_read) = tokio::io::duplex(4096);
        let (front_write, client_read) = tokio::io::duplex(4096);
        let (updates, update_rx) = mpsc::channel(1);
        let (event_tx, events) = mpsc::channel(8);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(Notify::new());
        let finish = Arc::new(Notify::new());
        let old = backend("old", false, calls.clone(), started.clone(), finish.clone());
        let front = tokio::spawn(relay(front_read, front_write, old, update_rx, event_tx));
        let mut f = Self { input: BufReader::new(client_read), output: client_write,
            updates, events, calls, started, finish, front };
        send(&mut f.output, json!({"jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-03-26", "capabilities":{},
                "clientInfo":{"name":"fixture", "version":"9.99.1"}}})).await;
        let init = receive(&mut f.input).await;
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["serverInfo"]["version"], "old");
        send(&mut f.output, json!({"jsonrpc":"2.0", "method":"notifications/initialized"})).await;
        assert!(matches!(f.event().await, Event::Serving { version, .. } if version == "old"));
        f
    }

    async fn event(&mut self) -> Event {
        tokio::time::timeout(Duration::from_secs(5), self.events.recv())
            .await.expect("bounded upgrade progress").expect("front remains live")
    }

    async fn update(&self, version: &str, fails: bool) {
        self.updates.send(backend(version, fails, self.calls.clone(),
            self.started.clone(), self.finish.clone())).await.unwrap();
    }

    async fn call(&mut self, id: u64) {
        send(&mut self.output, json!({"jsonrpc":"2.0", "id":id,
            "method":"tools/call", "params":{"name":"mutate"}})).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { self.front.abort(); }
}

// Real byte transports at the process boundary, with controlled backend
// responses. The front's framing, negotiation, draining and routing are live.
fn backend(version: &str, fails: bool, calls: Arc<Mutex<Vec<(String, Value)>>>,
    started: Arc<Notify>, finish: Arc<Notify>) -> Backend {
    let (front_in, server_in) = tokio::io::duplex(4096);
    let (mut server_out, front_out) = tokio::io::duplex(4096);
    let (front_control_in, control_in) = tokio::io::duplex(4096);
    let (mut control_out, front_control_out) = tokio::io::duplex(4096);
    let version = version.to_owned();
    tokio::spawn(async move {
        let mut lines = BufReader::new(server_in).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            let Some(id) = request.get("id").cloned() else { continue };
            if request.get("method").is_none() { continue; }
            let result = match request["method"].as_str().unwrap() {
                "initialize" if fails => {
                    send(&mut server_out, json!({"jsonrpc":"2.0", "id":id,
                        "error":{"code":-32603, "message":"candidate failed startup"}})).await;
                    break;
                }
                "initialize" => json!({"protocolVersion": if version == "protocol" { "2024-11-05" } else { "2025-03-26" },
                    "capabilities": if version == "capabilities" { json!({"tools":{"listChanged":true}}) } else { json!({"tools":{}}) },
                    "serverInfo":{"name":"cas", "version":version}}),
                "tools/list" if request["params"]["cursor"] == "second" => json!({"tools":[{
                    "name":"observe", "inputSchema": if version == "schema" {
                        json!({"type":"object", "required":["changed"]})
                    } else { json!({"type":"object"}) }}]}),
                "tools/list" => json!({"tools":[{"name":"mutate", "inputSchema":{"type":"object"}}], "nextCursor":"second"}),
                "ping" => json!({}),
                "tools/call" => {
                    calls.lock().await.push((version.clone(), id.clone()));
                    if id == 2 {
                        send(&mut server_out, json!({"jsonrpc":"2.0", "id":"server-work", "method":"roots/list"})).await;
                        started.notify_one();
                        finish.notified().await;
                        send(&mut server_out, json!({"jsonrpc":"2.0", "method":"notifications/progress",
                            "params":{"progressToken":"fixture", "progress":1}})).await;
                    }
                    json!({"version":version})
                }
                other => panic!("unexpected fixture method {other}"),
            };
            send(&mut server_out, json!({"jsonrpc":"2.0", "id":id, "result":result})).await;
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(control_in).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let request: Value = serde_json::from_str(&line).unwrap();
            send(&mut control_out, json!({"id":request["id"], "ok":true})).await;
        }
    });
    Backend { input: Box::new(front_in), output: Box::new(front_out),
        control_input: Box::new(front_control_in), control_output: Box::new(front_control_out), child: None, image: None, retry_after_drain: None }
}

#[tokio::test]
async fn failed_candidate_keeps_initialized_connection_serving_cas_81c4() {
    for (version, fails, reason) in [("broken", true, "candidate failed startup"),
        ("protocol", false, "protocolVersion"),
        ("capabilities", false, "capabilities differ"),
        ("schema", false, "tools/list schema differs for tool observe")] {
        let mut f = Fixture::new().await;
        f.update(version, fails).await;
        assert!(matches!(f.event().await, Event::Deferred(actual) if actual.contains(reason)));
        f.call(3).await;
        let reply = receive(&mut f.input).await;
        assert_eq!(reply["id"], 3);
        assert_eq!(reply["result"]["version"], "old");
        assert_eq!(*f.calls.lock().await, [("old".into(), json!(3))]);
    }
}

#[tokio::test]
async fn compatible_candidate_drains_inflight_and_partial_input_without_replay_cas_81c4() {
    let mut f = Fixture::new().await;
    f.call(2).await;
    f.started.notified().await;
    let peer_request = receive(&mut f.input).await;
    assert_eq!(peer_request["method"], "roots/list");
    f.output.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\"").await.unwrap();
    f.update("new", false).await;
    assert!(matches!(f.event().await, Event::Prepared));
    f.finish.notify_one();
    let progress = receive(&mut f.input).await;
    assert_eq!(progress["method"], "notifications/progress");
    let reply = receive(&mut f.input).await;
    assert_eq!(reply["id"], 2);
    assert_eq!(reply["result"]["version"], "old");
    f.output.write_all(b",\"params\":{\"name\":\"mutate\"}}\n").await.unwrap();
    let reply = receive(&mut f.input).await;
    assert_eq!(reply["id"], 3);
    assert_eq!(reply["result"]["version"], "old");
    send(&mut f.output, json!({"jsonrpc":"2.0", "id":peer_request["id"], "result":{"roots":[]}})).await;
    assert!(matches!(f.event().await, Event::Serving { version, .. } if version == "new"));
    f.call(4).await;
    let reply = receive(&mut f.input).await;
    assert_eq!(reply["id"], 4);
    assert_eq!(reply["result"]["version"], "new");
    assert_eq!(*f.calls.lock().await, [("old".into(), json!(2)),
        ("old".into(), json!(3)), ("new".into(), json!(4))]);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn drain_timeout_keeps_old_backend_and_pending_calls_cas_81c4() {
    let mut f = Fixture::new().await;
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("cas");
    std::fs::write(&program, "old").unwrap();
    let _serving_file = std::fs::File::open(&program).unwrap();
    let image = Arc::new(std::sync::Mutex::new(Some(
        crate::mcp::socket::ExeIdentity::from_exe_path(program.clone()))));
    let replacement = temp.path().join("replacement");
    std::fs::write(&replacement, "new").unwrap();
    std::fs::rename(&replacement, &program).unwrap();
    f.call(2).await;
    f.started.notified().await;
    let request = receive(&mut f.input).await;
    let (calls, started, finish) = (f.calls.clone(), f.started.clone(), f.finish.clone());
    let watcher = tokio::spawn(watch_updates(program, image, f.updates.clone(), move |_, _| {
        Ok(backend("new", false, calls.clone(), started.clone(), finish.clone()))
    }));
    assert!(matches!(f.event().await, Event::Prepared));
    tokio::time::advance(DRAIN_TIMEOUT + Duration::from_secs(1)).await;
    assert!(matches!(f.event().await, Event::Deferred(reason) if reason.contains("drain timed out")));
    f.finish.notify_one();
    assert_eq!(receive(&mut f.input).await["method"], "notifications/progress");
    assert_eq!(receive(&mut f.input).await["id"], 2);
    send(&mut f.output, json!({"jsonrpc":"2.0", "id":request["id"], "result":{"roots":[]}})).await;
    f.call(3).await;
    assert_eq!(receive(&mut f.input).await["result"]["version"], "old");
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(matches!(f.event().await, Event::Prepared), "same install must be retried after a drain deferral");
    assert!(matches!(f.event().await, Event::Serving { version, .. } if version == "new"));
    f.call(4).await;
    assert_eq!(receive(&mut f.input).await["result"]["version"], "new");
    assert_eq!(*f.calls.lock().await, [("old".into(), json!(2)), ("old".into(), json!(3)), ("new".into(), json!(4))]);
    watcher.abort();
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn deferred_binary_is_not_respawned_until_install_changes_cas_81c4() {
    let temp = tempfile::tempdir().unwrap();
    let program = temp.path().join("cas");
    std::fs::write(&program, "old").unwrap();
    // Keep the old inode alive, as an executing backend does on Unix.
    let _serving_file = std::fs::File::open(&program).unwrap();
    let serving = crate::mcp::socket::ExeIdentity::from_exe_path(program.clone());
    let image = std::sync::Arc::new(std::sync::Mutex::new(Some(serving)));
    let replacement = temp.path().join("replacement");
    std::fs::write(&replacement, "incompatible").unwrap();
    std::fs::rename(&replacement, &program).unwrap();
    let (updates, mut incoming) = mpsc::channel(1);
    let watcher = tokio::spawn(watch_updates(program.clone(), image, updates, |_, standby| {
        assert!(standby);
        Ok(backend("schema", false, Arc::default(), Arc::default(), Arc::default()))
    }));
    let first = incoming.recv().await.expect("first installed candidate");
    drop(first); // The relay rejected this image; the old one keeps serving.
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::task::yield_now().await;
    assert!(incoming.try_recv().is_err(), "deferred binary must not spawn every 30 seconds");
    std::fs::write(&replacement, "compatible").unwrap();
    std::fs::rename(&replacement, &program).unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    assert!(tokio::time::timeout(Duration::from_secs(31), incoming.recv()).await
        .expect("new install must be retried on the next tick").is_some());
    watcher.abort();
}
