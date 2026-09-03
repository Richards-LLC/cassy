use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use serde_json::Value;

fn read_jsonl(path: &std::path::Path) -> Vec<Value> {
    let data = std::fs::read_to_string(path).expect("jsonl should be written");
    data.lines()
        .map(|line| serde_json::from_str(line).expect("line should be valid json"))
        .collect()
}

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

fn write_slo_samples(
    root: &std::path::Path,
    normal_transport_ms: &[u64],
    urgent_transport_ms: &[u64],
    wake_ms: &[u64],
    reaction_ms: &[u64],
) {
    let receipts = root.join("receipts");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(
        receipts.join("slo_samples.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_type": "probe_comm_slo_samples",
            "provenance": "receipt:multi-sample conformance run",
            "normal_transport_ms": normal_transport_ms,
            "urgent_transport_ms": urgent_transport_ms,
            "wake_ms": wake_ms,
            "reaction_ms": reaction_ms
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn probe_comm_help_exposes_epic_slo_defaults() {
    cas_cmd()
        .args(["factory", "probe-comm", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--normal-transport-p95-slo-ms"))
        .stdout(predicates::str::contains("--normal-transport-max-slo-ms"))
        .stdout(predicates::str::contains("--urgent-transport-p95-slo-ms"))
        .stdout(predicates::str::contains("--urgent-transport-max-slo-ms"))
        .stdout(predicates::str::contains("--wake-p95-slo-ms"))
        .stdout(predicates::str::contains("--reaction-p95-slo-ms"))
        .stdout(predicates::str::contains("2000"))
        .stdout(predicates::str::contains("10000"))
        .stdout(predicates::str::contains("5000"))
        .stdout(predicates::str::contains("15000"));
}

#[test]
fn probe_comm_cli_writes_jsonl_with_fake_adapter() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");

    cas_cmd()
        .args(["factory", "probe-comm", "--jsonl"])
        .arg(&jsonl)
        .assert()
        .success();

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 7);
    assert_eq!(lines[0]["scenario"], "startup");
    assert_eq!(lines[1]["scenario"], "serial_10");
    assert_eq!(lines[1]["message_ids"].as_array().unwrap().len(), 10);
}

#[test]
fn probe_comm_cli_returns_nonzero_for_injected_failure() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--inject-transport-failure",
            "urgent:urgent-0",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("urgent").and(predicates::str::contains("delivered")));

    let lines = read_jsonl(&jsonl);
    let urgent = lines
        .iter()
        .find(|line| line["scenario"] == "urgent")
        .expect("urgent scenario should be written");
    assert_eq!(urgent["failed_stage"], "delivered");
}

/// GH #704: `probe-comm` leaked one disposable root per run into `$TMPDIR`
/// (1,242 of them on the operator host, which filled a 32 GB tmpfs and broke
/// every live session's shell output). Neither the success nor the failure
/// path may leave a root behind, and the generated default must live on disk
/// under `~/.cas/scratch/`, not in `$TMPDIR`.
#[test]
fn probe_comm_cli_removes_its_generated_root_on_success_and_failure() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let tmpdir = temp.path().join("tmp");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&tmpdir).unwrap();

    cas_cmd()
        .env("HOME", &home)
        .env("TMPDIR", &tmpdir)
        .args(["factory", "probe-comm", "--jsonl"])
        .arg(temp.path().join("pass.jsonl"))
        .assert()
        .success();

    cas_cmd()
        .env("HOME", &home)
        .env("TMPDIR", &tmpdir)
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            temp.path().join("fail.jsonl").to_str().unwrap(),
            "--inject-transport-failure",
            "urgent:urgent-0",
        ])
        .assert()
        .failure();

    for dir in [tmpdir.clone(), home.join(".cas").join("scratch")] {
        let leaked = probe_root_names(&dir);
        assert!(
            leaked.is_empty(),
            "probe-comm left {leaked:?} behind in {}",
            dir.display()
        );
    }
}

/// An operator-supplied `--cas-root` is theirs; the probe must not delete it.
#[test]
fn probe_comm_cli_preserves_an_explicit_cas_root() {
    let temp = tempfile::tempdir().unwrap();
    let explicit = temp.path().join("operator-root");

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            temp.path().join("probe.jsonl").to_str().unwrap(),
            "--cas-root",
            explicit.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(
        explicit.exists(),
        "an explicit --cas-root must survive the run"
    );
}

fn probe_root_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("cas-probe-comm-"))
        .collect()
}

#[test]
fn probe_comm_cli_all_adapters_writes_recorded_fixture_report() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);
    write_slo_samples(
        &artifacts,
        &[100, 300, 500, 800, 1200],
        &[100, 300, 500, 800, 1200],
        &[100, 800, 1500, 2500, 4000],
        &[100, 2000, 4000, 7000, 12000],
    );

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
    let scenarios: Vec<_> = lines
        .iter()
        .map(|line| line["scenario"].as_str().unwrap())
        .collect();
    assert!(
        scenarios.contains(&"routing_matrix_evidence"),
        "report must compose routing matrix evidence from cas-4484: {scenarios:?}"
    );
    assert!(
        scenarios.contains(&"merge_reclose_lifecycle_evidence"),
        "report must compose merge/re-close lifecycle evidence from cas-126b/cas-062d: {scenarios:?}"
    );
    assert!(
        lines
            .iter()
            .filter(|line| line["scenario"].as_str().unwrap().ends_with("_adapter"))
            .all(|line| line["passed"] == true)
    );
    assert!(
        lines
            .iter()
            .filter(|line| line["scenario"].as_str().unwrap().ends_with("_adapter"))
            .all(|line| line["stages"][0]["reaction_status"] == "OBSERVED")
    );
    assert!(
        lines
            .iter()
            .filter(|line| line["scenario"].as_str().unwrap().ends_with("_adapter"))
            .all(|line| line["stages"][0]["enqueued_at_ms"].is_null()
                && line["stages"][0]["selected_at_ms"].is_null()),
        "recorded adapters must not fabricate enqueue/select timestamps: {lines:?}"
    );
    let routing = lines
        .iter()
        .find(|line| line["scenario"] == "routing_matrix_evidence")
        .unwrap();
    assert_eq!(
        routing["stages"].as_array().unwrap().len(),
        18,
        "18 bidirectional harness pairings must come from receipts"
    );
    assert!(routing["passed"].as_bool().unwrap());
    assert!(
        routing["stages"].as_array().unwrap().iter().all(|stage| {
            stage["stage_statuses"][0]["provenance"]
                .as_str()
                .unwrap()
                .starts_with("receipt:")
        }),
        "routing observations must be receipt-backed: {routing}"
    );
    let lifecycle = lines
        .iter()
        .find(|line| line["scenario"] == "merge_reclose_lifecycle_evidence")
        .unwrap();
    assert!(lifecycle["passed"].as_bool().unwrap());
    assert!(
        lifecycle["stages"].as_array().unwrap().iter().any(|stage| {
            stage["message_id"] == "cas-ecff-lifecycle-outbox-recovery"
                && stage["stage_statuses"][0]["status"] == "OBSERVED"
        }),
        "lifecycle OBSERVED requires the merged cas-ecff receipt: {lifecycle}"
    );
    let slo = lines
        .iter()
        .find(|line| line["scenario"] == "slo_aggregate_evidence")
        .unwrap();
    assert_eq!(slo["passed"], true);
    assert_eq!(slo["slo_contract"]["normal_transport_p95_ms"], 2000);
    assert_eq!(slo["slo_contract"]["normal_transport_max_ms"], 10000);
    assert_eq!(slo["slo_contract"]["urgent_transport_p95_ms"], 2000);
    assert_eq!(slo["slo_contract"]["urgent_transport_max_ms"], 5000);
    assert_eq!(slo["slo_contract"]["wake_p95_ms"], 5000);
    assert_eq!(slo["slo_contract"]["reaction_p95_ms"], 15000);
    let aggregates = slo["aggregate_slos"].as_array().unwrap();
    assert!(
        aggregates
            .iter()
            .all(|agg| agg["sample_count"].as_u64().unwrap() >= 5)
    );
    assert!(
        aggregates
            .iter()
            .all(|agg| agg["provenance"] == "receipt:multi-sample conformance run")
    );
}

#[test]
fn probe_comm_cli_all_adapters_without_receipts_is_not_full_pass() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);

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
        .failure()
        .stderr(predicates::str::contains("routing_receipt_missing"));

    let lines = read_jsonl(&jsonl);
    let routing = lines
        .iter()
        .find(|line| line["scenario"] == "routing_matrix_evidence")
        .unwrap();
    assert_eq!(routing["passed"], false);
    assert_eq!(routing["failed_stage"], "routing_receipt_missing");
    assert!(
        routing["stages"].as_array().unwrap().iter().any(|stage| {
            stage["stage_statuses"][0]["status"] == "BLOCKED"
                || stage["stage_statuses"][0]["status"] == "UNKNOWN"
        }),
        "missing receipts must not create OBSERVED/PASS evidence: {routing}"
    );
}

#[test]
fn probe_comm_cli_malformed_receipt_is_stage_failure() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    let receipts = artifacts.join("receipts");
    std::fs::create_dir_all(&receipts).unwrap();
    std::fs::write(receipts.join("routing_matrix.json"), "{not-json}\n").unwrap();

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
        .failure()
        .stderr(predicates::str::contains("routing_receipt_malformed"));

    let lines = read_jsonl(&jsonl);
    let routing = lines
        .iter()
        .find(|line| line["scenario"] == "routing_matrix_evidence")
        .unwrap();
    assert_eq!(routing["failed_stage"], "routing_receipt_malformed");
    assert_eq!(
        routing["stages"][0]["stage_statuses"][0]["status"],
        "FAILED"
    );
}

#[test]
fn probe_comm_cli_slo_aggregate_uses_nearest_rank_p95_for_five_samples() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);
    write_slo_samples(
        &artifacts,
        &[100, 300, 500, 800, 2500],
        &[100, 300, 500, 800, 1200],
        &[100, 800, 1500, 2500, 4000],
        &[100, 2000, 4000, 7000, 12000],
    );

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
        .failure()
        .stderr(predicates::str::contains("normal_transport_p95"));

    let lines = read_jsonl(&jsonl);
    let slo = lines
        .iter()
        .find(|line| line["scenario"] == "slo_aggregate_evidence")
        .unwrap();
    assert_eq!(slo["failed_stage"], "normal_transport_p95");
    let aggregates = slo["aggregate_slos"].as_array().unwrap();
    let p95 = aggregates
        .iter()
        .find(|agg| agg["metric"] == "normal_transport" && agg["gate"] == "p95")
        .unwrap();
    assert_eq!(p95["observed_ms"], 2500);
    assert_eq!(p95["passed"], false);
    let max = aggregates
        .iter()
        .find(|agg| agg["metric"] == "normal_transport" && agg["gate"] == "max")
        .unwrap();
    assert_eq!(max["observed_ms"], 2500);
    assert_eq!(max["passed"], true);
}

#[test]
fn probe_comm_cli_slo_aggregate_receipt_enforces_epic_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);

    let cases = [
        (
            "normal_transport_p95",
            vec![100, 300, 500, 2500, 2600],
            vec![100, 300, 500, 800, 1200],
            vec![100, 800, 1500, 2500, 4000],
            vec![100, 2000, 4000, 7000, 12000],
        ),
        (
            "normal_transport_max",
            {
                let mut samples = vec![800; 19];
                samples.push(12000);
                samples
            },
            vec![100, 300, 500, 800, 1200],
            vec![100, 800, 1500, 2500, 4000],
            vec![100, 2000, 4000, 7000, 12000],
        ),
        (
            "urgent_transport_p95",
            vec![100, 300, 500, 800, 1200],
            vec![100, 300, 500, 2500, 2600],
            vec![100, 800, 1500, 2500, 4000],
            vec![100, 2000, 4000, 7000, 12000],
        ),
        (
            "urgent_transport_max",
            vec![100, 300, 500, 800, 1200],
            {
                let mut samples = vec![800; 19];
                samples.push(6000);
                samples
            },
            vec![100, 800, 1500, 2500, 4000],
            vec![100, 2000, 4000, 7000, 12000],
        ),
        (
            "wake_p95",
            vec![100, 300, 500, 800, 1200],
            vec![100, 300, 500, 800, 1200],
            vec![100, 800, 1500, 6000, 7000],
            vec![100, 2000, 4000, 7000, 12000],
        ),
        (
            "reaction_p95",
            vec![100, 300, 500, 800, 1200],
            vec![100, 300, 500, 800, 1200],
            vec![100, 800, 1500, 2500, 4000],
            vec![100, 2000, 4000, 16000, 17000],
        ),
    ];

    for (failed_stage, normal, urgent, wake, reaction) in cases {
        write_slo_samples(&artifacts, &normal, &urgent, &wake, &reaction);
        let jsonl = temp.path().join(format!("{failed_stage}.jsonl"));
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
            .failure()
            .stderr(predicates::str::contains(failed_stage));

        let lines = read_jsonl(&jsonl);
        let slo = lines
            .iter()
            .find(|line| line["scenario"] == "slo_aggregate_evidence")
            .unwrap();
        assert_eq!(slo["failed_stage"], failed_stage);
    }
}

#[test]
fn probe_comm_cli_slo_aggregate_requires_distribution_samples() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);
    write_slo_samples(&artifacts, &[100], &[100], &[100], &[100]);

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
        .failure()
        .stderr(predicates::str::contains("slo_sample_count_insufficient"));

    let lines = read_jsonl(&jsonl);
    let slo = lines
        .iter()
        .find(|line| line["scenario"] == "slo_aggregate_evidence")
        .unwrap();
    assert_eq!(slo["passed"], false);
    assert_eq!(slo["failed_stage"], "slo_sample_count_insufficient");
}

#[test]
fn probe_comm_cli_malformed_slo_aggregate_receipt_is_stage_failure() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    write_composed_receipts(&artifacts);
    let receipts = artifacts.join("receipts");
    std::fs::write(receipts.join("slo_samples.json"), "{not-json}\n").unwrap();

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
        .failure()
        .stderr(predicates::str::contains("slo_receipt_malformed"));

    let lines = read_jsonl(&jsonl);
    let slo = lines
        .iter()
        .find(|line| line["scenario"] == "slo_aggregate_evidence")
        .unwrap();
    assert_eq!(slo["passed"], false);
    assert_eq!(slo["failed_stage"], "slo_receipt_malformed");
    assert_eq!(slo["stages"][0]["stage_statuses"][0]["status"], "FAILED");
}

#[test]
fn probe_comm_cli_recorded_adapter_reaction_sample_is_observation_only() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "codex",
            "--artifact-root",
            artifacts.to_str().unwrap(),
            "--reaction-p95-slo-ms",
            "1",
        ])
        .assert()
        .success();

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["scenario"], "codex_adapter");
    assert_eq!(lines[0]["passed"], true);
    assert_eq!(lines[0]["failed_stage"], Value::Null);
    assert_eq!(lines[0]["stages"][0]["terminal"], "delivered");
    assert_eq!(lines[0]["stages"][0]["reaction_status"], "OBSERVED");
    assert_eq!(lines[0]["aggregate_slos"], Value::Null);
    assert!(
        lines[0]["stages"][0]["stage_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| !stage.to_string().contains("p95")),
        "a single reaction observation must not be labeled as p95: {}",
        lines[0]
    );
}

#[test]
fn probe_comm_cli_recorded_adapter_wake_sample_is_observation_only() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "codex",
            "--artifact-root",
            artifacts.to_str().unwrap(),
            "--wake-p95-slo-ms",
            "1",
            "--reaction-p95-slo-ms",
            "1000",
        ])
        .assert()
        .success();

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["scenario"], "codex_adapter");
    assert_eq!(lines[0]["passed"], true);
    assert_eq!(lines[0]["failed_stage"], Value::Null);
    assert_eq!(lines[0]["stages"][0]["terminal"], "delivered");
    assert_eq!(lines[0]["stages"][0]["wake_status"], "OBSERVED");
    assert_eq!(lines[0]["aggregate_slos"], Value::Null);
    assert!(
        lines[0]["stages"][0]["stage_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| !stage.to_string().contains("p95")),
        "a single wake observation must not be labeled as p95: {}",
        lines[0]
    );
}

#[test]
fn probe_comm_cli_fake_adapter_applies_delivery_and_selection_thresholds_independently() {
    let temp = tempfile::tempdir().unwrap();
    let delivery_jsonl = temp.path().join("delivery.jsonl");
    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            delivery_jsonl.to_str().unwrap(),
            "--inject-slo-failure",
            "serial_10:serial-0:50",
            "--delivery-slo-ms",
            "1",
            "--selection-slo-ms",
            "1000",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("delivery_slo"));

    let selection_jsonl = temp.path().join("selection.jsonl");
    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            selection_jsonl.to_str().unwrap(),
            "--delivery-slo-ms",
            "1000",
            "--selection-slo-ms",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("selected"));

    let lines = read_jsonl(&selection_jsonl);
    assert_eq!(lines[0]["failed_stage"], "selected");
}

#[test]
fn probe_comm_cli_malformed_recorded_artifact_emits_stage_status_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    let codex = artifacts.join("codex");
    std::fs::create_dir_all(&codex).unwrap();
    std::fs::write(codex.join("rollout.jsonl"), "{not-json}\n").unwrap();

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "codex",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("artifact_parse_failed"));

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["scenario"], "codex_adapter");
    assert_eq!(lines[0]["passed"], false);
    assert_eq!(lines[0]["failed_stage"], "artifact_parse_failed");
    assert_eq!(lines[0]["stages"][0]["terminal"], "artifact_parse_failed");
    assert_eq!(lines[0]["stages"][0]["delivered_status"], "FAILED");
    assert!(
        lines[0]["stages"][0]["stage_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| stage["stage"] == "artifact" && stage["status"] == "FAILED")
    );
}

#[test]
fn probe_comm_cli_malformed_claude_transcript_is_not_swallowed() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    std::fs::write(
        artifacts.join("claude").join("transcript.jsonl"),
        "{not-json}\n",
    )
    .unwrap();

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "claude",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("transcript_parse_failed"));

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines[0]["scenario"], "claude_adapter");
    assert_eq!(lines[0]["failed_stage"], "transcript_parse_failed");
    assert_eq!(lines[0]["stages"][0]["terminal"], "transcript_parse_failed");
}

#[test]
fn probe_comm_cli_malformed_grok_events_is_not_swallowed() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    write_adapter_artifacts(&artifacts);
    let grok = artifacts.join("grok");
    write_jsonl(
        &grok.join("updates.jsonl"),
        &[
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.000Z","type":"user_message","text":"probe-message-id=grok-1"}),
            serde_json::json!({"timestamp":"2026-07-21T17:00:02.100Z","type":"turn_started"}),
        ],
    );
    std::fs::write(grok.join("events.jsonl"), "{not-json}\n").unwrap();

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "grok",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("events_parse_failed"));

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines[0]["scenario"], "grok_adapter");
    assert_eq!(lines[0]["failed_stage"], "events_parse_failed");
    assert_eq!(lines[0]["stages"][0]["terminal"], "events_parse_failed");
}

#[test]
fn probe_comm_cli_missing_recorded_artifact_emits_stage_status_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "codex",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("artifact_missing"));

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["scenario"], "codex_adapter");
    assert_eq!(lines[0]["failed_stage"], "artifact_missing");
    assert_eq!(lines[0]["stages"][0]["terminal"], "artifact_missing");
    assert_eq!(lines[0]["stages"][0]["delivered_status"], "FAILED");
}

#[test]
fn probe_comm_cli_unmatched_recorded_artifact_emits_unknown_stage_status_jsonl() {
    let temp = tempfile::tempdir().unwrap();
    let jsonl = temp.path().join("probe.jsonl");
    let artifacts = temp.path().join("artifacts");
    let codex = artifacts.join("codex");
    std::fs::create_dir_all(&codex).unwrap();
    write_jsonl(
        &codex.join("rollout.jsonl"),
        &[serde_json::json!({
            "timestamp":"2026-07-21T17:00:02.000Z",
            "type":"event_msg",
            "payload":{"type":"user_message","message":"probe-message-id=other"}
        })],
    );

    cas_cmd()
        .args([
            "factory",
            "probe-comm",
            "--jsonl",
            jsonl.to_str().unwrap(),
            "--adapter",
            "codex",
            "--artifact-root",
            artifacts.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("correlation_unknown"));

    let lines = read_jsonl(&jsonl);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["scenario"], "codex_adapter");
    assert_eq!(lines[0]["failed_stage"], "correlation_unknown");
    assert_eq!(lines[0]["stages"][0]["terminal"], "correlation_unknown");
    assert_eq!(lines[0]["stages"][0]["delivered_status"], "UNKNOWN");
    assert!(
        lines[0]["stages"][0]["stage_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stage| stage["stage"] == "artifact" && stage["status"] == "UNKNOWN")
    );
}
