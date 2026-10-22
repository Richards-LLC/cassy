//! Read-back: a journey's durable effects checked after it runs, shared by
//! post-release and the release gate.
//!
//! A journey declares which subject kinds and invariant IDs it reads back.
//! Cassy forwards only those subject names, their opaque ids and the declared
//! invariant IDs, never a query. Results take the post-release [`Readback`]
//! shape; observed values are never kept.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One read-back result. Post-release probes report it directly; the gate
/// builds it from the adapter's `readback` verb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Readback {
    pub journey: String,
    pub subject: String,
    pub invariants: Vec<String>,
    pub ok: bool,
}

impl Readback {
    /// A passing read-back that names its journey, subject and invariants.
    pub fn proves(&self) -> bool {
        self.ok && !self.journey.is_empty() && !self.subject.is_empty() && !self.invariants.is_empty()
    }
}

/// A journey's `readback:` declaration in the registry.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Declaration {
    /// Subject kinds, for example `account_uid` or `workspace_id`.
    pub subjects: Vec<String>,
    /// Invariant IDs, for example `INV-WORKSPACE`.
    pub invariants: Vec<String>,
    /// Unknown keys, kept so registry validation can name a typo.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl<'de> Deserialize<'de> for Declaration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Flatten's intermediate content rejects YAML tags before the value
        // type sees them. Read unknown YAML values directly from the map.
        struct DeclarationVisitor;
        impl<'de> serde::de::Visitor<'de> for DeclarationVisitor {
            type Value = Declaration;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a readback declaration")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Declaration, A::Error> {
                let (mut subjects, mut invariants) = (None, None);
                let mut extra = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "subjects" => {
                            if subjects.is_some() {
                                return Err(serde::de::Error::duplicate_field("subjects"));
                            }
                            subjects = Some(map.next_value()?);
                        }
                        "invariants" => {
                            if invariants.is_some() {
                                return Err(serde::de::Error::duplicate_field("invariants"));
                            }
                            invariants = Some(map.next_value()?);
                        }
                        _ => {
                            extra.insert(key, map.next_value()?);
                        }
                    }
                }
                Ok(Declaration {
                    subjects: subjects.unwrap_or_default(),
                    invariants: invariants.unwrap_or_default(),
                    extra,
                })
            }
        }
        deserializer.deserialize_map(DeclarationVisitor)
    }
}

impl Declaration {
    /// Registry validation: typos and malformed lists fail at `--check`
    /// instead of at gate time.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .extra
            .keys()
            .map(|key| format!("readback has unknown field {key:?}; use subjects and invariants"))
            .collect();
        for (field, values) in [("subjects", &self.subjects), ("invariants", &self.invariants)] {
            if values.is_empty() {
                problems.push(format!("readback.{field} must not be empty"));
            }
            for value in values.iter().filter(|value| !name(value)) {
                problems.push(format!("readback.{field} entry {value:?} is not an identifier"));
            }
        }
        problems
    }
}

/// What the gate does with one read-back.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Every declared invariant read back ok.
    Pass(Readback),
    /// At least one invariant read back false: a product defect.
    Fail { readback: Readback, failed: Vec<String> },
    /// A provider failed, throttled or ran out (`external`, `rate_limited`,
    /// `insufficient`): the row stays owed.
    BlockedExternal(crate::qa_release::blocked::BlockedExternal),
    /// No complete answer, so the row cannot pass.
    Missing(String),
}

const MAX_ID: usize = 128;

/// Opaque ids are short tokens. Whitespace, quotes and SQL punctuation are
/// refused, so a query cannot pass for an id.
fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'@'))
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// The allow-listed `readback` verb arguments, or why the request is refused
/// before any adapter call.
pub fn request_args<'a>(
    journey: &str,
    declaration: Option<&'a Declaration>,
    subjects: &BTreeMap<String, String>,
) -> Result<(&'a Declaration, Value), String> {
    let declaration = declaration
        .filter(|d| !d.subjects.is_empty() && !d.invariants.is_empty())
        .ok_or_else(|| format!("journey {journey} declares no readback"))?;
    if let Some(bad) = declaration.invariants.iter().find(|id| !name(id)) {
        return Err(format!(
            "journey {journey} readback invariant {:?} is not an invariant ID",
            bad.chars().take(40).collect::<String>()
        ));
    }
    if subjects.is_empty() {
        return Err(format!("journey {journey} readback has no subject ids"));
    }
    for (kind, id) in subjects {
        if !declaration.subjects.contains(kind) {
            return Err(format!(
                "subject {:?} is not in journey {journey}'s readback allow-list",
                kind.chars().take(40).collect::<String>()
            ));
        }
        if !opaque_id(id) {
            return Err(format!("subject {kind} is not an opaque id"));
        }
    }
    let args = serde_json::json!({
        "subjects": subjects,
        "invariants": declaration.invariants,
    });
    Ok((declaration, args))
}

/// Maps the adapter's `{invariant_id: {ok, observed}}` answer, or its error,
/// to an [`Outcome`]. Any false invariant fails; an unanswered one cannot pass.
pub fn classify(
    journey: &str,
    declaration: &Declaration,
    subjects: &BTreeMap<String, String>,
    answer: Result<Value, crate::qa_release::CallError>,
) -> Outcome {
    use crate::qa_release::{CallError, blocked::BlockedExternal};
    if declaration.invariants.is_empty() {
        return Outcome::Missing(format!("journey {journey} declares no readback invariants"));
    }
    let data = match answer {
        Ok(data) => data,
        Err(error) => {
            if let CallError::Adapter(_, adapter) = &error
                && let Some(blocked) = BlockedExternal::from_adapter(adapter)
            {
                return Outcome::BlockedExternal(blocked);
            }
            return Outcome::Missing(error.to_string());
        }
    };
    let readback = |ok| Readback {
        journey: journey.into(),
        subject: subjects
            .iter()
            .map(|(kind, id)| format!("{kind}={id}"))
            .collect::<Vec<_>>()
            .join(" "),
        invariants: declaration.invariants.clone(),
        ok,
    };
    let answered = |id: &str| data.get(id).and_then(|r| r.get("ok")).and_then(Value::as_bool);
    let failed: Vec<String> = declaration
        .invariants
        .iter()
        .filter(|id| answered(id) == Some(false))
        .cloned()
        .collect();
    if !failed.is_empty() {
        return Outcome::Fail {
            readback: readback(false),
            failed,
        };
    }
    let missing: Vec<&str> = declaration
        .invariants
        .iter()
        .filter(|id| answered(id).is_none())
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        Outcome::Pass(readback(true))
    } else {
        Outcome::Missing(format!("not read back: {}", missing.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Malformed inputs fail where they are written: component paths at config
    /// parse, `readback:` blocks at registry check, and an empty declaration
    /// never classifies as PASS.
    #[test]
    fn readback_declarations_and_components_fail_early() {
        use crate::qa_release::ReleaseConfig;
        for path in ["../shared", "apps/../../etc", ":(glob)apps/**", ":!apps/web"] {
            let error =
                ReleaseConfig::parse_components(&format!(r#"{{"backend":["{path}"]}}"#))
                    .unwrap_err();
            assert!(error.contains("qa.release.components.backend path"), "{error}");
        }
        assert!(ReleaseConfig::parse_components(r#"{"backend":["apps/backend"]}"#).is_ok());

        let registry = crate::qa_registry::parse(
            "journeys:\n  - id: signup\n    status: dark\n    status_reason: later\n    readback: {subject: [account_uid], invariants: [\"SELECT 1\"]}\n",
        )
        .unwrap();
        let problems: Vec<String> = registry
            .validate(&[])
            .problems
            .into_iter()
            .filter(|p| p.code == "readback")
            .map(|p| p.message)
            .collect();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("unknown field \"subject\""), "{problems:?}");

        let empty = Declaration::default();
        let answer = Ok(serde_json::json!({}));
        assert!(matches!(
            classify("signup", &empty, &BTreeMap::new(), answer),
            Outcome::Missing(_)
        ));
    }
}
