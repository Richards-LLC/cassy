//! Verification gates E2E tests
//!
//! Tests the verification system: adding verifications, pending checks, quality gates

use crate::fixtures::new_cas_instance;
use std::process::Command;

/// Test adding a verification to a task
#[test]
fn test_add_verification() {
    let cas = new_cas_instance();

    // Create and start a task
    let task_id = cas.create_task("Task to verify");
    cas.start_task(&task_id);

    // Add a verification
    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "Code review passed",
            "--confidence",
            "0.9",
        ])
        .output()
        .expect("Failed to add verification");

    assert!(
        output.status.success(),
        "verification add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test listing verifications for a task
#[test]
fn test_list_verifications() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task with multiple verifications");
    cas.start_task(&task_id);

    // Add multiple verifications
    for i in 1..=3 {
        let output = cas
            .cas_cmd()
            .args([
                "verification",
                "add",
                &task_id,
                "--status",
                "approved",
                "--summary",
                &format!("Verification round {}", i),
            ])
            .output()
            .expect("Failed to add verification");
        assert!(output.status.success());
    }

    // List verifications
    let output = cas
        .cas_cmd()
        .args(["verification", "list", &task_id])
        .output()
        .expect("Failed to list verifications");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should show all verifications
    assert!(stdout.contains("approved"));
}

/// Test getting the latest verification for a task
#[test]
fn test_latest_verification() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task for latest verification");
    cas.start_task(&task_id);

    // Add verifications
    cas.cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "First review",
        ])
        .output()
        .expect("Failed to add first verification");

    cas.cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "rejected",
            "--summary",
            "Second review found issues",
        ])
        .output()
        .expect("Failed to add second verification");

    // Get latest verification
    let output = cas
        .cas_cmd()
        .args(["verification", "latest", &task_id])
        .output()
        .expect("Failed to get latest verification");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Latest should be the rejected one
    assert!(
        stdout.contains("rejected"),
        "Latest verification should be the second one"
    );
}

/// Test verification status types
#[test]
fn test_verification_statuses() {
    let cas = new_cas_instance();

    let statuses = ["approved", "rejected", "error", "skipped"];

    for status in statuses {
        let task_id = cas.create_task(&format!("Task for {} verification", status));
        cas.start_task(&task_id);

        let output = cas
            .cas_cmd()
            .args([
                "verification",
                "add",
                &task_id,
                "--status",
                status,
                "--summary",
                &format!("Verification with {} status", status),
            ])
            .output()
            .expect("Failed to add verification");

        assert!(
            output.status.success(),
            "verification add with status {} failed: {}",
            status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Test verification with issues
#[test]
fn test_verification_with_issues() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task with issues");
    cas.start_task(&task_id);

    // Add verification with issues as JSON
    let issues_json = r#"[{"severity":"error","message":"Missing tests"},{"severity":"warning","message":"Consider refactoring"}]"#;

    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "rejected",
            "--summary",
            "Found issues during review",
            "--issues",
            issues_json,
        ])
        .output()
        .expect("Failed to add verification with issues");

    assert!(
        output.status.success(),
        "verification add with issues failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Show verification
    let output = cas
        .cas_cmd()
        .args(["verification", "latest", &task_id])
        .output()
        .expect("Failed to get verification");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain issue information
    println!("Verification with issues: {}", stdout);
}

/// Test verification with files reviewed
#[test]
fn test_verification_with_files() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task with file review");
    cas.start_task(&task_id);

    // Add verification with files
    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "Reviewed all implementation files",
            "--files",
            "src/main.rs,src/lib.rs,tests/test.rs",
        ])
        .output()
        .expect("Failed to add verification with files");

    assert!(
        output.status.success(),
        "verification add with files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Test pending verification detection
#[test]
fn test_pending_verification_detection() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task needing verification");
    cas.start_task(&task_id);

    // Set pending_verification flag via direct DB update (simulating task close request)
    let db_path = cas.temp_dir.path().join(".cas").join("cas.db");
    let _ = Command::new("sqlite3")
        .arg(&db_path)
        .arg(format!(
            "UPDATE tasks SET pending_verification = 1 WHERE id = '{}';",
            task_id
        ))
        .output();

    // Check pending verifications
    let output = cas
        .cas_cmd()
        .args(["verification", "pending"])
        .output()
        .expect("Failed to check pending verifications");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should list the task as pending verification
    assert!(
        stdout.contains(&task_id),
        "Task should appear in pending verifications"
    );
}

/// Test confidence score in verification
#[test]
fn test_verification_confidence_score() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task for confidence test");
    cas.start_task(&task_id);

    // Add verification with specific confidence
    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "High confidence approval",
            "--confidence",
            "0.95",
        ])
        .output()
        .expect("Failed to add verification");

    assert!(output.status.success());

    // Get the verification and check confidence
    let output = cas
        .cas_cmd()
        .args(["verification", "latest", &task_id, "--json"])
        .output()
        .expect("Failed to get verification");

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(conf) = v.get("confidence").and_then(|c| c.as_f64()) {
                assert!(
                    (conf - 0.95).abs() < 0.01,
                    "Confidence should be ~0.95, got {}",
                    conf
                );
            }
        }
    }
}

/// Test showing a specific verification by ID
#[test]
fn test_show_verification() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task for show verification");
    cas.start_task(&task_id);

    // Add verification and capture ID
    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "Test verification for show",
        ])
        .output()
        .expect("Failed to add verification");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Extract verification ID if present
    let re = regex::Regex::new(r"(verif-[a-f0-9]+)").ok();
    if let Some(re) = re {
        if let Some(caps) = re.captures(&stdout) {
            let verif_id = caps.get(1).map(|m| m.as_str()).unwrap_or("");

            if !verif_id.is_empty() {
                // Show the specific verification
                let output = cas
                    .cas_cmd()
                    .args(["verification", "show", verif_id])
                    .output()
                    .expect("Failed to show verification");

                assert!(output.status.success());
            }
        }
    }
}

/// Test verification after task close attempt
#[test]
fn test_verification_after_close_attempt() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task to close and verify");
    cas.start_task(&task_id);

    // Add progress notes
    cas.add_task_note(&task_id, "Implementation complete", "progress");

    // Attempt to close (this may trigger verification requirement)
    let _output = cas
        .cas_cmd()
        .args(["task", "close", &task_id])
        .output()
        .expect("Failed to close task");

    // Check if task was closed or if verification is pending
    let task = cas.get_task_json(&task_id);
    let status = task["status"].as_str().unwrap_or("");
    let pending_v = task
        .get("pending_verification")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    println!(
        "Task status after close: {}, pending_verification: {}",
        status, pending_v
    );

    // Either closed successfully or pending verification
    assert!(
        status == "closed" || pending_v,
        "Task should either be closed or pending verification"
    );
}

/// Test clearing pending verification
#[test]
fn test_clear_pending_verification() {
    let cas = new_cas_instance();

    let task_id = cas.create_task("Task with pending verification to clear");
    cas.start_task(&task_id);

    // Set pending_verification
    let db_path = cas.temp_dir.path().join(".cas").join("cas.db");
    let _ = Command::new("sqlite3")
        .arg(&db_path)
        .arg(format!(
            "UPDATE tasks SET pending_verification = 1 WHERE id = '{}';",
            task_id
        ))
        .output();

    // Add approved verification
    let output = cas
        .cas_cmd()
        .args([
            "verification",
            "add",
            &task_id,
            "--status",
            "approved",
            "--summary",
            "Approved after review",
        ])
        .output()
        .expect("Failed to add verification");

    assert!(output.status.success());

    // The approved verification should clear pending_verification
    // Check the task
    let task = cas.get_task_json(&task_id);

    // Note: This depends on implementation - verification approval may or may not
    // automatically clear pending_verification flag
    println!("Task after approved verification: {:?}", task);
}

/// Test multiple tasks with pending verifications
#[test]
fn test_multiple_pending_verifications() {
    let cas = new_cas_instance();

    let db_path = cas.temp_dir.path().join(".cas").join("cas.db");

    // Create multiple tasks and set them as pending verification
    let task_ids: Vec<String> = (1..=3)
        .map(|i| {
            let task_id = cas.create_task(&format!("Pending task {}", i));
            cas.start_task(&task_id);
            let _ = Command::new("sqlite3")
                .arg(&db_path)
                .arg(format!(
                    "UPDATE tasks SET pending_verification = 1 WHERE id = '{}';",
                    task_id
                ))
                .output();
            task_id
        })
        .collect();

    // Check pending verifications
    let output = cas
        .cas_cmd()
        .args(["verification", "pending"])
        .output()
        .expect("Failed to list pending verifications");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // All tasks should appear in pending list
    for task_id in &task_ids {
        assert!(
            stdout.contains(task_id),
            "Task {} should be in pending list",
            task_id
        );
    }
}
