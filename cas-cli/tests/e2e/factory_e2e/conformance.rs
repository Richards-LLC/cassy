//! Recorded-artifact conformance coverage for `cas factory probe-comm`.
//!
//! This lives under `factory_e2e` to pin the factory conformance surface without
//! launching real model processes in normal test runs. Live disposable model
//! probes remain an explicit future gate; recorded fixtures are deterministic.

use assert_cmd::Command;
use serde_json::Value;

fn cas_cmd() -> Command {
    Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ))
}

fn write_jsonl(path: &std::path::Path, values: &[Value]) {
    let body = values
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, format!("{body}\n")).unwrap();
}

fn read_jsonl(path: &std::path::Path) -> Vec<Value> {
    let data = std::fs::read_to_string(path).expect("jsonl should be written");
    data.lines()
        .map(|line| serde_json::from_str(line).expect("line should be valid json"))
        .collect()
}

fn write_adapter_artifacts(root: &std::path::Path) {
    let claude = root.join("claude");
    let codex = root.join("codex");
    let grok = root.join("grok");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::create_dir_all(&grok).unwrap();

    std::fs::write(
        claude.join("inbox.json"),
        serde_json::to_string_pretty(&serde_json::json!([
            {
                "from": "supervisor",
                "text": "probe-message-id=claude-1 please reply",
                "summary": "probe claude-1",
                "timestamp": "2026-07-21T17:00:01.000Z",
                "color": "green",
                "read": false
            }
        ]))
        .unwrap(),
    )
    .unwrap();
    write_jsonl(
        &claude.join("transcript.jsonl"),
        &[
            serde_json::json!({"timestamp":"2026-07-21T17:00:01.200Z","type":"assistant","message":"ack probe-message-id=claude-1"}),
        ],
    );
    write_jsonl(
        &codex.join("rollout.jsonl"),
        &[
            serde_json::json!({"timestamp":"2026-07-21T17:00:00.000Z","type":"session_meta","payload":{"cwd":"/tmp/probe"}}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.000Z","type":"event_msg","payload":{"type":"user_message","message":"probe-message-id=codex-1"}}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.100Z","type":"event_msg","payload":{"type":"turn_started"}}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.200Z","type":"response_item","payload":{"type":"message","content":"ack probe-message-id=codex-1"}}),
        ],
    );
    write_jsonl(
        &grok.join("updates.jsonl"),
        &[
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.000Z","type":"user_message","text":"probe-message-id=grok-1"}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.100Z","type":"turn_started"}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.200Z","type":"assistant_message","text":"ack probe-message-id=grok-1"}),
        ],
    );
    write_jsonl(
        &grok.join("events.jsonl"),
        &[
            serde_json::json!({"ts":"2026-07-21T17:00:08.000Z","type":"turn_ended","outcome":"completed"}),
        ],
    );
}

fn write_composed_receipts(root: &std::path::Path) {
    let receipts = root.join("receipts");
    std::fs::create_dir_all(&receipts).unwrap();
    let harnesses = ["claude", "codex", "grok"];
    let mut contracts = Vec::new();
    for supervisor in harnesses {
        for worker in harnesses {
            contracts.push(serde_json::json!({
                "message_id": format!("{supervisor}-supervisor-to-{worker}-worker"),
                "target": format!("{worker}-worker"),
                "stage": "routing_matrix",
                "status": "OBSERVED",
                "provenance": "receipt:delivery_matrix_all_combos_both_directions"
            }));
            contracts.push(serde_json::json!({
                "message_id": format!("{worker}-worker-to-{supervisor}-supervisor"),
                "target": format!("{supervisor}-supervisor"),
                "stage": "routing_matrix",
                "status": "OBSERVED",
                "provenance": "receipt:delivery_matrix_all_combos_both_directions"
            }));
        }
    }
    std::fs::write(
        receipts.join("routing_matrix.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_type": "routing_matrix",
            "contracts": contracts
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        receipts.join("lifecycle.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_type": "merge_reclose_lifecycle",
            "receipts": [
                {
                    "message_id": "cas-126b-merge-reclose-halt-exemption",
                    "target": "awaiting-merge-worker",
                    "stage": "merge_reclose",
                    "status": "OBSERVED",
                    "provenance": "receipt:bounded re-close urgent halt exemption"
                },
                {
                    "message_id": "cas-062d-owner-lifecycle-transitions",
                    "target": "owning-supervisor",
                    "stage": "lifecycle_transition",
                    "status": "OBSERVED",
                    "provenance": "receipt:owner lifecycle transition push"
                },
                {
                    "message_id": "cas-ecff-lifecycle-outbox-recovery",
                    "target": "owning-supervisor",
                    "stage": "lifecycle_outbox_recovery",
                    "status": "OBSERVED",
                    "provenance": "receipt:exactly-once lifecycle outbox recovery"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_slo_samples(root: &std::path::Path) {
    let receipts = root.join("receipts");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(
        receipts.join("slo_samples.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_type": "probe_comm_slo_samples",
            "provenance": "receipt:multi-sample conformance run",
            "normal_transport_ms": [100, 300, 500, 800, 1200],
            "urgent_transport_ms": [100, 300, 500, 800, 1200],
            "wake_ms": [100, 800, 1500, 2500, 4000],
            "reaction_ms": [100, 2000, 4000, 7000, 12000]
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn recorded_probe_comm_all_adapters_emit_stage_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);
    write_slo_samples(&artifacts);

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "all",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .success();

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 6);
    assert!(lines.iter().all(|line| line["passed"] == true));
    assert!(
        lines
            .iter()
            .filter(|line| line["scenario"].as_str().unwrap().ends_with("_adapter"))
            .all(|line| line["stages"][0]["reaction_status"] == "OBSERVED")
    );
    assert!(
        lines
            .iter()
            .any(|line| line["scenario"] == "routing_matrix_evidence")
    );
    assert!(
        lines
            .iter()
            .any(|line| line["scenario"] == "merge_reclose_lifecycle_evidence")
    );
    assert!(
        lines
            .iter()
            .any(|line| line["scenario"] == "slo_aggregate_evidence")
    );
}
