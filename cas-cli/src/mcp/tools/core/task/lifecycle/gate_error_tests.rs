use super::{TaskLifecycleGateError, close_ops::epic_close_owner_gate};

#[test]
fn epic_close_owner_gate_exposes_owner_mismatch_as_a_typed_error() {
    let error = epic_close_owner_gate(
        "cas-epic",
        "owner-id",
        Some("other-id"),
        Some("other-name"),
        Some("other-session"),
    )
    .expect_err("a different authenticated caller must not close an owned epic");

    assert!(matches!(
        &error,
        TaskLifecycleGateError::OwnerMismatch { .. }
    ));
}
