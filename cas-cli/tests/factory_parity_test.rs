//! CI-safe integration test for the launched-agent skill + instruction parity
//! conformance gate (cas-bd9d). Runs the real `cas factory parity` command and
//! asserts the machine-readable 3×2 report proves required skills/agents,
//! effective role clauses, and tool prefix for every harness/role cell from the
//! real launch configuration — with no live model process (the faithful,
//! deterministic path). This is the Demo: "run one parity command and receive a
//! 3×2 report".

use assert_cmd::Command;
use serde_json::Value;

fn cas_cmd() -> Command {
    Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ))
}

#[test]
fn factory_parity_command_reports_all_six_cells_passing() {
    let out = cas_cmd()
        .args(["factory", "parity"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).expect("stdout must be a JSON report");

    assert_eq!(report["all_passed"], true, "every cell must pass: {report}");
    let cells = report["cells"].as_array().expect("cells array");
    assert_eq!(cells.len(), 6, "matrix must be 3 harnesses × 2 roles");

    // Every (harness, role) shape is present and passes all five named stages.
    let mut shapes: Vec<String> = Vec::new();
    for cell in cells {
        let harness = cell["harness"].as_str().unwrap();
        let role = cell["role"].as_str().unwrap();
        shapes.push(format!("{harness}/{role}"));
        assert_eq!(cell["passed"], true, "{harness}/{role} failed: {cell}");
        let stage_names: Vec<&str> = cell["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["stage"].as_str().unwrap())
            .collect();
        assert_eq!(
            stage_names,
            vec![
                "catalog",
                "instruction_source",
                "launch_args",
                "effective_contract",
                "prefix"
            ],
            "{harness}/{role} must name all four+prefix stages"
        );
        for stage in cell["stages"].as_array().unwrap() {
            assert_eq!(
                stage["status"], "PASS",
                "{harness}/{role} stage {} not PASS: {stage}",
                stage["stage"]
            );
        }
    }
    shapes.sort();
    assert_eq!(
        shapes,
        vec![
            "claude/supervisor",
            "claude/worker",
            "codex/supervisor",
            "codex/worker",
            "grok/supervisor",
            "grok/worker",
        ]
    );

    // Composes with (does not duplicate) the communication conformance gate.
    assert!(
        report["composes_with"]
            .as_str()
            .unwrap()
            .contains("probe-comm"),
        "report must link the sibling communication conformance surface"
    );
}

#[test]
fn factory_parity_writes_per_cell_jsonl() {
    let tmp = tempfile::tempdir().unwrap();
    let jsonl = tmp.path().join("parity.jsonl");
    cas_cmd()
        .args(["factory", "parity", "--jsonl", jsonl.to_str().unwrap()])
        .assert()
        .success();

    let body = std::fs::read_to_string(&jsonl).expect("jsonl written");
    let lines: Vec<Value> = body
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is one cell JSON object"))
        .collect();
    assert_eq!(lines.len(), 6, "one JSONL line per matrix cell");
    assert!(lines.iter().all(|c| c["passed"] == true));
}
