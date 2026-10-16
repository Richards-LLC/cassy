//! Consume Cloud-owned per-binding fence evidence; a v2 claim is not activation.
use super::*;
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub(crate) struct BindingEvidence {
    pub managed: BTreeSet<(String, String)>,
    pub legacy: BTreeSet<(String, String)>,
}

impl BindingEvidence {
    pub fn require_managed(&self, workspace: &str, channel: &str) -> Result<(), String> {
        if self.managed.contains(&(workspace.into(), channel.into())) {
            Ok(())
        } else {
            Err("strict activation is not evidenced for this Slack binding".into())
        }
    }
}

pub(crate) fn validate_activation(_response: &Value, _context: &OperationContext) -> Result<BindingEvidence, String> {
    Err("binding activation evidence is not implemented".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(index: usize) -> Value {
        let value: Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/coordination_contract/v1/bindings--activation.json"
        )).unwrap();
        value["cases"][index]["response"]["payload"].clone()
    }

    fn context() -> OperationContext {
        OperationContext { project_id: "github.com/richards-llc/cassy".into(), registration_id: "reg-supervisor-beta-08".into() }
    }

    #[test]
    fn cas_c423_binding_evidence_accepts_strict_fence_and_keeps_unmanaged_v1_separate() {
        let evidence = validate_activation(&fixture(0), &context()).unwrap();
        evidence.require_managed("T07ABCDEF", "C08COORD01").unwrap();
        assert!(evidence.require_managed("T07ABCDEF", "C08LEGACY1").is_err());
        assert!(evidence.legacy.contains(&("T07ABCDEF".into(), "C08LEGACY1".into())));
        let refused = validate_activation(&fixture(1), &context()).unwrap();
        assert!(refused.require_managed("T07ABCDEF", "C08COORD01").is_err());
        assert!(refused.legacy.contains(&("T07ABCDEF".into(), "C08COORD01".into())));
        assert!(validate_activation(&fixture(2), &context()).is_err());
    }

    #[test]
    fn cas_c423_binding_evidence_refuses_missing_fences_foreign_projects_and_live_old_consumers() {
        // Start with actual evidenced output and change only the contract
        // fact that would make managed intake unsafe.
        for (pointer, invalid) in [
            ("/bindings/0/fence_receipt", Value::Null),
            ("/bindings/0/v1/late_ack", json!("accepted")),
            ("/bindings/0/fence_receipt/fence_generation", json!(99)),
            ("/bindings/0/fence_receipt/quiescence/blocking_v1_consumers", json!(["old-sweep-client"])),
            ("/bindings/0/v1/outstanding/unexpired_leases", json!(1)),
            ("/bindings/0/binding/project_id", json!("github.com/foreign/project")),
        ] {
            let mut response = fixture(0);
            *response.pointer_mut(pointer).unwrap() = invalid;
            assert!(validate_activation(&response, &context()).is_err(), "accepted {pointer}");
        }
        let mut duplicate = fixture(0);
        let row = duplicate["bindings"][0].clone();
        duplicate["bindings"].as_array_mut().unwrap().push(row);
        assert!(validate_activation(&duplicate, &context()).is_err(), "ambiguous duplicate binding");
    }
}
