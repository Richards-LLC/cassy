use crate::hooks::handlers::*;

// =========================================================================
// detect_and_mark_skill_drift tests (cas-f9ad)
// =========================================================================

fn skill_project(body: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let cas_root = tmp.path().join(".cas");
    let skill = tmp.path().join(".claude/skills/cas-worker/SKILL.md");
    std::fs::create_dir_all(&cas_root).unwrap();
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, body).unwrap();
    (tmp, cas_root, skill)
}

/// Initial SessionStart already reads the disk file, so the first fingerprint
/// is recorded without asking the harness to reload the same bytes.
#[test]
fn skill_drift_initial_load_records_disk_fingerprint_without_reload_cas_0efb() {
    let (_tmp, cas_root, _skill) = skill_project("initial guidance\n");
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-a"));
    let marker = cas_root.join("session_skills_seen_sess-drift-a");
    let fingerprint = std::fs::read_to_string(marker).unwrap();
    assert!(fingerprint.starts_with("sha256:"), "{fingerprint}");
}

#[test]
fn skill_drift_unchanged_disk_file_is_quiet_cas_0efb() {
    let (_tmp, cas_root, _skill) = skill_project("same guidance\n");
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-b"));
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-b"));
}

#[test]
fn skill_drift_empty_session_id_does_not_create_bare_marker_cas_0efb() {
    let (_tmp, cas_root, _skill) = skill_project("guidance\n");

    assert!(!detect_and_mark_skill_drift(&cas_root, ""));
    assert!(
        !cas_root.join("session_skills_seen_").exists(),
        "empty session ids must never create a bare marker"
    );
}

/// A correction made directly to the working-tree file is detected without a
/// sentinel or remote-ref change; after acknowledgement it is idempotent.
#[test]
fn skill_drift_detects_working_tree_file_change_then_quiets_cas_0efb() {
    let (_tmp, cas_root, skill) = skill_project("unsafe old claim\n");
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-d"));
    std::fs::write(skill, "corrected safety guidance\n").unwrap();
    assert!(detect_and_mark_skill_drift(&cas_root, "sess-drift-d"));
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-d"));
}

/// Different sessions establish independent initial fingerprints. A session
/// that starts after the edit already loaded the corrected bytes and should
/// not receive a redundant reload.
#[test]
fn skill_drift_independent_per_session_cas_0efb() {
    let (_tmp, cas_root, skill) = skill_project("v1\n");
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-e1"));
    std::fs::write(skill, "v2\n").unwrap();
    assert!(detect_and_mark_skill_drift(&cas_root, "sess-drift-e1"));
    assert!(!detect_and_mark_skill_drift(&cas_root, "sess-drift-e2"));
}

// =========================================================================
// reloadSkills JSON serialization tests (cas-f9ad)
// =========================================================================

/// `with_reload_skills(true)` on an existing SessionStart output adds
/// `"reloadSkills":true` to the JSON wire shape.
#[test]
fn reload_skills_true_serializes() {
    let output = HookOutput::with_session_start_context("ctx".into()).with_reload_skills(true);
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("\"reloadSkills\":true"),
        "Expected reloadSkills:true in: {json}"
    );
    assert!(
        json.contains("\"additionalContext\":\"ctx\""),
        "additionalContext must still be present: {json}"
    );
}

/// `with_reload_skills(true)` on an empty output creates a minimal SessionStart
/// output with an empty additionalContext.
#[test]
fn reload_skills_on_empty_output_creates_session_start() {
    let output = HookOutput::empty().with_reload_skills(true);
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("\"reloadSkills\":true"),
        "Expected reloadSkills:true in: {json}"
    );
    assert!(
        json.contains("SessionStart"),
        "Expected SessionStart hookEventName: {json}"
    );
}

/// When `reload_skills` is `None` (default), the field is absent from JSON.
#[test]
fn reload_skills_absent_by_default() {
    let output = HookOutput::with_session_start_context("ctx".into());
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        !json.contains("reloadSkills"),
        "reloadSkills must be absent when not set: {json}"
    );
}
