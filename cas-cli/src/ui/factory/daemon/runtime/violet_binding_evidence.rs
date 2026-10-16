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

pub(crate) fn validate_activation(
    response: &Value,
    context: &OperationContext,
) -> Result<BindingEvidence, String> {
    require_completed(response)?;
    let rows = response["bindings"]
        .as_array()
        .ok_or("activation has no binding list")?;
    let mut evidence = BindingEvidence::default();
    let mut identities = BTreeSet::new();
    let mut channels = BTreeSet::new();
    for row in rows {
        let binding = &row["binding"];
        let id = binding["id"].as_str().ok_or("binding has no identity")?;
        uuid::Uuid::parse_str(id).map_err(|_| "invalid binding identity")?;
        let workspace = binding["workspace_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("binding has no workspace")?;
        let channel = binding["channel_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("binding has no channel")?;
        let key = (workspace.to_string(), channel.to_string());
        if binding["project_id"].as_str() != Some(context.project_id.as_str())
            || binding["provider"] != "slack"
            || !identities.insert(id.to_string())
            || !channels.insert(key.clone())
        {
            return Err("foreign, unsupported or ambiguous activation binding".into());
        }
        match binding["mode"].as_str() {
            Some("legacy") => {
                if !matches!(
                    row["strict_activation"].as_str(),
                    Some("not_requested" | "refused")
                ) || row["v1"]["publication"] != "open"
                    || row["v1"]["leasing"] != "open"
                    || row["v1"]["late_ack"] != "accepted"
                {
                    return Err("legacy binding carries contradictory fence evidence".into());
                }
                evidence.legacy.insert(key);
            }
            Some("managed") => {
                let fence = &row["fence_receipt"];
                let quiescence = &fence["quiescence"];
                let generation = binding["fence_generation"].as_u64().filter(|g| *g > 0);
                let fence_id = fence["id"]
                    .as_str()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok());
                let timestamps = ["activated_at", "legacy_fenced_at"].iter().all(|field| {
                    binding[*field]
                        .as_str()
                        .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
                });
                if row["strict_activation"] != "evidenced"
                    || binding["verified"] != true
                    || generation.is_none()
                    || fence["fence_generation"].as_u64() != generation
                    || fence_id.is_none()
                    || !timestamps
                    || !matches!(
                        fence["outcome"].as_str(),
                        Some("activated" | "already_managed")
                    )
                    || row["v1"]["publication"] != "fenced"
                    || row["v1"]["leasing"] != "fenced"
                    || row["v1"]["late_ack"] != "refused"
                    || !["pending", "leased", "unexpired_leases"]
                        .iter()
                        .all(|field| row["v1"]["outstanding"][*field].as_u64() == Some(0))
                    || !quiescence["quiet_window_secs"]
                        .as_u64()
                        .is_some_and(|secs| (120..=3600).contains(&secs))
                    || quiescence["blocking_v1_leases"].as_u64() != Some(0)
                    || !quiescence["blocking_v1_consumers"]
                        .as_array()
                        .is_some_and(Vec::is_empty)
                    || quiescence["attested_v1_consumers"].as_array().is_none()
                    || ![
                        "migrated_v1_rows",
                        "retired_v1_rows",
                        "invalidated_v1_leases",
                    ]
                    .iter()
                    .all(|field| fence["settled"][*field].as_u64().is_some())
                {
                    return Err(
                        "strict activation lacks current v1 fence/quiescence evidence".into(),
                    );
                }
                evidence.managed.insert(key);
            }
            _ => return Err("unsupported binding activation mode".into()),
        }
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(index: usize) -> Value {
        let value: Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/coordination_contract/v1/bindings--activation.json"
        ))
        .unwrap();
        value["cases"][index]["response"]["payload"].clone()
    }

    fn context() -> OperationContext {
        OperationContext {
            project_id: "github.com/richards-llc/cassy".into(),
            registration_id: "reg-supervisor-beta-08".into(),
        }
    }

    #[test]
    fn cas_c423_binding_evidence_accepts_strict_fence_and_keeps_unmanaged_v1_separate() {
        let evidence = validate_activation(&fixture(0), &context()).unwrap();
        evidence.require_managed("T07ABCDEF", "C08COORD01").unwrap();
        assert!(evidence.require_managed("T07ABCDEF", "C08LEGACY1").is_err());
        assert!(
            evidence
                .legacy
                .contains(&("T07ABCDEF".into(), "C08LEGACY1".into()))
        );
        let refused = validate_activation(&fixture(1), &context()).unwrap();
        assert!(refused.require_managed("T07ABCDEF", "C08COORD01").is_err());
        assert!(
            refused
                .legacy
                .contains(&("T07ABCDEF".into(), "C08COORD01".into()))
        );
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
            (
                "/bindings/0/fence_receipt/quiescence/blocking_v1_consumers",
                json!(["old-sweep-client"]),
            ),
            ("/bindings/0/v1/outstanding/unexpired_leases", json!(1)),
            (
                "/bindings/0/binding/project_id",
                json!("github.com/foreign/project"),
            ),
        ] {
            let mut response = fixture(0);
            *response.pointer_mut(pointer).unwrap() = invalid;
            assert!(
                validate_activation(&response, &context()).is_err(),
                "accepted {pointer}"
            );
        }
        let mut duplicate = fixture(0);
        let row = duplicate["bindings"][0].clone();
        duplicate["bindings"].as_array_mut().unwrap().push(row);
        assert!(
            validate_activation(&duplicate, &context()).is_err(),
            "ambiguous duplicate binding"
        );
    }
}
