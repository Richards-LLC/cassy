//! Real-world search scoring tests for CAS use cases
//!
//! These tests verify that the search scoring algorithm produces
//! meaningful, well-calibrated scores for typical CAS queries.
//!
//! ## Search Tests
//!
//! Search uses BM25 text-based matching. Local semantic search has been
//! removed in favor of cloud-based semantic search.

use std::process::Command;
use tempfile::TempDir;

/// Check if embedding model is available for testing
/// Always returns false - local embeddings have been removed
fn embeddings_available() -> bool {
    false
}

/// Skip test if embeddings aren't available (use in non-ignored tests)
/// Local embeddings have been removed - these tests are always skipped
macro_rules! skip_without_embeddings {
    () => {
        if !embeddings_available() {
            eprintln!("Skipping: local embeddings have been removed (cloud-only)");
            return;
        }
    };
}

/// Helper to run CAS commands in a temp directory
fn cas_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(cas::test_paths::binary(
        "cas",
        option_env!("CARGO_BIN_EXE_cas").map(Into::into),
    ));
    let home = dir.path().join(".test-home");
    let xdg = dir.path().join(".test-xdg-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg).unwrap();
    if let Some(host_home) = std::env::var_os("HOME") {
        cmd.env("CAS_TEST_PROTECTED_HOME", host_home);
    }
    cmd.env("HOME", home).env("XDG_CONFIG_HOME", xdg);
    cmd.env("CAS_DIR", dir.path());
    cmd.env("CAS_SKIP_FACTORY_TOOLING", "1");
    cmd.current_dir(dir.path());
    cmd
}

/// Initialize CAS in a temp directory
fn init_cas(dir: &TempDir) {
    let output = cas_cmd(dir)
        .args(["init"])
        .output()
        .expect("Failed to run cas init");
    assert!(output.status.success(), "cas init failed");
}

/// Add a memory entry with optional title and tags
fn add_memory(dir: &TempDir, content: &str, title: Option<&str>, tags: Option<&str>) -> String {
    let mut cmd = cas_cmd(dir);
    cmd.arg("add");

    if let Some(t) = title {
        cmd.args(["--title", t]);
    }
    if let Some(t) = tags {
        cmd.args(["--tags", t]);
    }
    cmd.arg(content);

    let output = cmd.output().expect("Failed to add memory");
    assert!(
        output.status.success(),
        "cas add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Parse the ID from the output
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find(|l| l.contains("Added entry:"))
        .and_then(|l| l.split(':').next_back())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Generate embeddings for semantic search
/// Always returns false - local embeddings have been removed
fn generate_embeddings(_dir: &TempDir) -> bool {
    false
}

/// Search with full hybrid (BM25 + semantic) - returns results with scores
fn search_hybrid(dir: &TempDir, query: &str) -> Vec<(String, f64)> {
    let output = cas_cmd(dir)
        .args(["search", "--json", query])
        .output()
        .expect("Failed to search");

    if !output.status.success() {
        return Vec::new();
    }

    parse_search_results(&String::from_utf8_lossy(&output.stdout))
}

/// Search using BM25 only (no semantic)
fn search_bm25(dir: &TempDir, query: &str) -> Vec<(String, f64)> {
    let output = cas_cmd(dir)
        .args(["search", "--json", "--no-semantic", query])
        .output()
        .expect("Failed to search");

    if !output.status.success() {
        return Vec::new();
    }

    parse_search_results(&String::from_utf8_lossy(&output.stdout))
}

/// Parse JSON search results
fn parse_search_results(stdout: &str) -> Vec<(String, f64)> {
    let mut results = Vec::new();
    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(arr) = v.as_array() {
                for item in arr {
                    if let (Some(id), Some(score)) = (
                        item.get("id").and_then(|v| v.as_str()),
                        item.get("score").and_then(|v| v.as_f64()),
                    ) {
                        results.push((id.to_string(), score));
                    }
                }
            }
        }
    }
    results
}

// ============================================================================
// Real-World CAS Use Case Tests
// ============================================================================

/// Test: Exact keyword match should score highly
#[path = "search_scoring_test_cases/tests.rs"]
mod tests;
