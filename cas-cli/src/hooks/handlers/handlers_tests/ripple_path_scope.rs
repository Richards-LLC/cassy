//! Tests for ripple-check path-awareness (cas-9aeb).
//!
//! Ripple-check was firing cross-project false positives: editing a file in
//! project A surfaced tasks from project B because the matcher did a pure
//! substring search ("CLAUDE.md" matches every task that mentions CLAUDE.md).
//!
//! Fix: before any task-store query, confirm the edited file lives inside the
//! current project's root directory.  Files outside the project boundary are
//! silently suppressed.

use crate::hooks::handlers::is_file_within_project;

// ── is_file_within_project unit tests ────────────────────────────────────────

#[test]
fn file_inside_project_root_is_within() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path();
    let file = project_root.join("src").join("main.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "").unwrap();

    assert!(
        is_file_within_project(&file, project_root),
        "file under project root must be within the project"
    );
}

#[test]
fn file_at_project_root_itself_is_within() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path();
    let file = project_root.join("CLAUDE.md");
    std::fs::write(&file, "").unwrap();

    assert!(
        is_file_within_project(&file, project_root),
        "file at the project root level must be within the project"
    );
}

#[test]
fn file_in_sibling_directory_is_not_within() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Two sibling projects share the same parent temp dir.
    let project_a = tmp.path().join("project_a");
    let project_b = tmp.path().join("project_b");
    std::fs::create_dir_all(&project_a).unwrap();
    std::fs::create_dir_all(&project_b).unwrap();

    // File lives in project_b but we check against project_a's root.
    let file = project_b.join("CLAUDE.md");
    std::fs::write(&file, "").unwrap();

    assert!(
        !is_file_within_project(&file, &project_a),
        "file in a sibling project must NOT be within the other project"
    );
}

#[test]
fn nonexistent_file_outside_project_is_not_within() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().join("project_a");
    std::fs::create_dir_all(&project_root).unwrap();

    // File path that doesn't exist and is outside the project.
    let outside = tmp.path().join("other").join("CLAUDE.md");

    assert!(
        !is_file_within_project(&outside, &project_root),
        "non-existent file outside project must NOT be within the project"
    );
}

#[test]
fn nonexistent_file_inside_project_is_within() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project_root = tmp.path().canonicalize().expect("canonical project root");

    // File that doesn't exist yet, but its path is inside the project.
    let file = project_root.join("src").join("lib.rs");

    assert!(
        is_file_within_project(&file, &project_root),
        "non-existent file whose path is under project root must be within the project"
    );
}
