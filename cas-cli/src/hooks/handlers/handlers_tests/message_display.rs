//! Tests for cas-b39a: MessageDisplay hook — Ink-crash guard + assistant-text
//! redaction (opt-in).
//!
//! Four required test cases:
//!   1. opt-in + nested-fence input → sanitized (no inner ``` block left)
//!   2. opt-in + secret in message → redacted (secret not present in output)
//!   3. default-off → byte-identical passthrough (output JSON == "{}")
//!   4. plain prose → untouched (no transform applied)

use cas_core::hooks::types::HookInput;
use std::fs;

use crate::hooks::handlers::handlers_events::handle_message_display;

// ============================================================================
// Helpers
// ============================================================================

/// Build a MessageDisplay `HookInput` by **parsing the real wire payload**.
///
/// cas-f3e3: this helper used to build the struct by hand with
/// `message: Some(...)`. That is why all four tests below stayed green while
/// the handler was unreachable in production — Claude Code sends the assistant
/// text under `delta`, never `message`, so `input.message` was `None` on every
/// real event and `handle_message_display` returned on its first line.
///
/// The envelope below is a verbatim capture from Claude Code 2.1.224. Keeping
/// these tests on the parse path is the point: a struct literal cannot catch a
/// deserialization-contract bug, and this file is the proof.
fn md_input(message: &str) -> HookInput {
    let payload = serde_json::json!({
        "session_id": "test-session",
        "transcript_path": "/test/transcript.jsonl",
        "cwd": "/test",
        "prompt_id": "d323a56b-73b1-4afc-8b3f-32605be51b91",
        "hook_event_name": "MessageDisplay",
        "turn_id": "83db6fbe-820e-4db4-8845-d5515e9158c3",
        "message_id": "513a75fd-35f5-451b-a7ed-b925c5ecc4d9",
        "index": 0,
        "final": true,
        "delta": message,
    });
    serde_json::from_value(payload).expect("real MessageDisplay payload must deserialize")
}

/// The guard must be unreachable for a non-final streaming chunk: a fence or a
/// secret spanning a chunk boundary cannot be judged from one chunk.
#[test]
fn non_final_chunk_is_passthrough_even_with_the_guard_on() {
    let dir = tempfile::tempdir().unwrap();
    let cas_root = write_guard_config(dir.path());

    let partial: HookInput = serde_json::from_value(serde_json::json!({
        "session_id": "test-session",
        "cwd": "/test",
        "hook_event_name": "MessageDisplay",
        "index": 0,
        "final": false,
        "delta": "leaked token: sk-abcdef1234567890abcdef1234567890",
    }))
    .unwrap();

    let out = handle_message_display(&partial, Some(&cas_root)).unwrap();
    assert_eq!(
        serde_json::to_string(&out).unwrap(),
        "{}",
        "a partial chunk must pass through untouched"
    );
}

/// Create a `.cas` subdirectory inside `dir`, write
/// `[hooks] message_display_guard = true`, and return the `.cas` path.
///
/// Callers pass the returned path as `cas_root` to `handle_message_display`
/// so `Config::load(cas_root)` resolves `cas_root/config.toml`.
fn write_guard_config(dir: &std::path::Path) -> std::path::PathBuf {
    let cas_dir = dir.join(".cas");
    fs::create_dir_all(&cas_dir).unwrap();
    let config_toml = "[hooks]\nmessage_display_guard = true\n";
    fs::write(cas_dir.join("config.toml"), config_toml).unwrap();
    cas_dir
}

// ============================================================================
// Test 1: opt-in + nested fenced code block → sanitized
// ============================================================================

#[test]
fn optin_nested_fence_is_sanitized() {
    // A markdown block containing another fence is the primary Ink crash trigger.
    // The outer fence encloses markdown content that itself has a fenced block.
    let input_text = concat!(
        "Here is some code:\n",
        "```markdown\n",
        "# Heading\n",
        "```rust\n",
        "fn main() {}\n",
        "```\n",
        "End of example.\n",
        "```\n",
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_dir = write_guard_config(tmp.path());

    let input = md_input(input_text);
    let output = handle_message_display(&input, Some(&cas_dir)).expect("handler ok");

    // The handler must have returned a transformed message (not empty passthrough).
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("updatedMessage"),
        "Expected updatedMessage in output JSON when guard is on and nested fence detected:\n{json}"
    );

    // The inner ``` should be escaped/replaced — there must be no raw nested triple-backtick fence.
    // Extract the updatedMessage value and check.
    let val: serde_json::Value = serde_json::from_str(&json).unwrap();
    let updated = val
        .pointer("/hookSpecificOutput/updatedMessage")
        .and_then(|v| v.as_str())
        .expect("hookSpecificOutput.updatedMessage must be a string");

    // The sanitized output must not contain a triple-backtick on its own line
    // inside another triple-backtick block (detect the simplest nested-fence shape).
    // We check that the raw pattern "```\n```" (inner closing fence immediately
    // before outer closing fence) is gone.
    assert!(
        !has_nested_fence(updated),
        "Sanitized output must not contain nested triple-backtick fences:\n{updated}"
    );
}

/// Returns true if `text` contains a labeled ``` fence inside another ``` fence.
///
/// Uses the same depth-tracking logic as the production `has_nested_fence` in
/// `handlers_events/message_display.rs`: bare ``` at depth > 0 is a closer;
/// labeled ``` at depth > 0 is a nested opener.
fn has_nested_fence(text: &str) -> bool {
    let mut depth: usize = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("```") {
            continue;
        }
        let after = trimmed["```".len()..].trim();
        if depth == 0 {
            depth = 1;
        } else if after.is_empty() {
            depth = depth.saturating_sub(1);
        } else {
            // Labeled ``` inside a fence — nested!
            return true;
        }
    }
    false
}

// ============================================================================
// Test 2: opt-in + secret pattern in message → redacted
// ============================================================================

#[test]
fn optin_secret_in_message_is_redacted() {
    // Embed a plausible secret token pattern. The handler should detect and
    // replace it with a redaction placeholder.
    let secret = "sk-1234567890abcdef1234567890abcdef";
    let input_text = format!(
        "Use the following API key to authenticate:\n{secret}\nThat's all.\n"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_dir = write_guard_config(tmp.path());

    let input = md_input(&input_text);
    let output = handle_message_display(&input, Some(&cas_dir)).expect("handler ok");

    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("updatedMessage"),
        "Expected updatedMessage in output JSON when guard is on and secret detected:\n{json}"
    );

    // The secret must not appear verbatim in the transformed output.
    let val: serde_json::Value = serde_json::from_str(&json).unwrap();
    let updated = val
        .pointer("/hookSpecificOutput/updatedMessage")
        .and_then(|v| v.as_str())
        .expect("hookSpecificOutput.updatedMessage must be a string");

    assert!(
        !updated.contains(secret),
        "Secret must be redacted from the transformed output.\nGot: {updated}"
    );
}

// ============================================================================
// Test 3: default-off (no config) → byte-identical passthrough
// ============================================================================

#[test]
fn default_off_is_byte_identical_passthrough() {
    // No config written → guard is OFF (default false).
    // The handler must return HookOutput::empty() → JSON `{}`.
    let tmp = tempfile::tempdir().expect("tempdir");
    // Do NOT write a config file — use defaults.

    let input_text = "```markdown\n```rust\nfn main() {}\n```\n```\n";
    let input = md_input(input_text);
    let output = handle_message_display(&input, Some(tmp.path())).expect("handler ok");

    let json = serde_json::to_string(&output).unwrap();
    assert_eq!(
        json, "{}",
        "Default-off guard must return empty JSON (byte-identical passthrough), got: {json}"
    );
}

// ============================================================================
// Test 4: opt-in + plain prose → untouched (no transform)
// ============================================================================

#[test]
fn optin_plain_prose_is_untouched() {
    // Plain prose with no nested fences and no secret patterns.
    // Even with the guard enabled, the handler must NOT transform benign content.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cas_dir = write_guard_config(tmp.path());

    let input_text = "Here is a simple explanation of the algorithm.\nIt runs in O(n log n) time.\n";
    let input = md_input(input_text);
    let output = handle_message_display(&input, Some(&cas_dir)).expect("handler ok");

    let json = serde_json::to_string(&output).unwrap();
    assert_eq!(
        json, "{}",
        "Plain prose must not be transformed even with guard enabled, got: {json}"
    );
}
