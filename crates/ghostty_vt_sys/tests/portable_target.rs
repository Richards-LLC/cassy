#[path = "../build_support.rs"]
mod build_support;

use build_support::rust_target_to_zig;

#[test]
fn native_release_targets_are_pinned_to_zig_baselines() {
    assert_eq!(
        rust_target_to_zig("x86_64-unknown-linux-gnu"),
        Ok("x86_64-linux-gnu")
    );
    assert_eq!(
        rust_target_to_zig("aarch64-apple-darwin"),
        Ok("aarch64-macos")
    );
}

#[test]
fn supported_cross_targets_keep_their_abi() {
    assert_eq!(
        rust_target_to_zig("x86_64-unknown-linux-musl"),
        Ok("x86_64-linux-musl")
    );
    assert_eq!(
        rust_target_to_zig("aarch64-unknown-linux-gnu"),
        Ok("aarch64-linux-gnu")
    );
}

#[test]
fn unknown_targets_fail_closed_with_actionable_diagnostics() {
    let error = rust_target_to_zig("riscv64gc-unknown-linux-gnu").unwrap_err();
    assert!(error.contains("refusing to build"), "{error}");
    assert!(error.contains("riscv64gc-unknown-linux-gnu"), "{error}");
    assert!(error.contains("x86_64-unknown-linux-gnu"), "{error}");
}
