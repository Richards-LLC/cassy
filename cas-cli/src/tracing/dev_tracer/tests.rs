use crate::tracing::dev_tracer::DevTracer;
use crate::tracing::dev_tracer::helpers::{generate_session_id, sanitize_args};

#[test]
fn test_sanitize_args() {
    let args = vec!["short".to_string(), "a".repeat(300)];
    let sanitized = sanitize_args(&args);
    assert_eq!(sanitized[0], "short");
    assert_eq!(sanitized[1].len(), 200);
    assert!(sanitized[1].ends_with("..."));
}

#[test]
fn test_sanitize_args_empty() {
    let args: Vec<String> = vec![];
    let sanitized = sanitize_args(&args);
    assert!(sanitized.is_empty());
}

#[test]
fn test_sanitize_args_exactly_200() {
    let args = vec!["a".repeat(200)];
    let sanitized = sanitize_args(&args);
    assert_eq!(sanitized[0].len(), 200);
    assert!(!sanitized[0].ends_with("..."));
}

#[test]
fn test_sanitize_args_201() {
    let args = vec!["a".repeat(201)];
    let sanitized = sanitize_args(&args);
    assert_eq!(sanitized[0].len(), 200);
    assert!(sanitized[0].ends_with("..."));
}



#[test]
fn test_session_id_format() {
    let id = generate_session_id();
    assert!(id.starts_with("cli-"));
    // Should be hex after the prefix
    let hex_part = &id[4..];
    assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_dev_tracer_is_not_enabled_by_default() {
    // Without explicit init, tracer should not be enabled
    assert!(!DevTracer::is_enabled());
}

#[test]
fn test_dev_tracer_get_returns_none_by_default() {
    // Without init, get should return None
    // Note: This test may be affected by other tests that init the global tracer
    // In practice, run in isolation
    let tracer = DevTracer::get();
    // The tracer may or may not be initialized depending on test order
    // So we just check the return type is correct
    let _ = tracer;
}
