use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde_json::{Value, json};

use super::{EndSessionReceipt, LeaseRefusal, SessionsResponse};
use crate::hub::{HubSession, LeaseSummary, MachineEvent};
use crate::ui::factory::{DaemonWelcome, SessionEndOutcome, supervisor_commands::SupervisorCommandReceipt};

fn serializer_fields(schema: &mut Schema) {
    schema.remove("description");
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    let required = object.get("required").cloned();
    let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for (name, field) in properties {
        if required.as_ref().and_then(Value::as_array).is_some_and(|names| {
            names.iter().any(|required_name| required_name.as_str() == Some(name.as_str()))
        }) {
            continue;
        }
        if let Some(Value::Array(types)) = field.get_mut("type") {
            types.retain(|kind| kind != "null");
            if types.len() == 1 {
                field["type"] = types.pop().unwrap();
            }
        }
        if let Some(Value::Array(variants)) = field.get_mut("anyOf") {
            variants.retain(|variant| variant != &json!({"type": "null"}));
        }
    }
}

fn serializer_schema<Wire: JsonSchema>() -> Schema {
    let mut schema = SchemaSettings::draft2020_12()
        .for_serialize()
        .with_transform(RecursiveTransform(serializer_fields))
        .into_generator()
        .into_root_schema_for::<Wire>();
    schema.insert(
        "$id".into(),
        json!(format!("urn:cassy:hub:{}", Wire::schema_name())),
    );
    schema
}

#[test]
fn hub_double_matches_serializer_schema() {
    let generated = json!({
        "DaemonWelcome": serializer_schema::<DaemonWelcome>(),
        "EndSessionReceipt": serializer_schema::<EndSessionReceipt>(),
        "HubSession": serializer_schema::<HubSession>(),
        "LeaseRefusal": serializer_schema::<LeaseRefusal>(),
        "LeaseSummary": serializer_schema::<LeaseSummary>(),
        "MachineEvent": serializer_schema::<MachineEvent>(),
        "SessionEndOutcome": serializer_schema::<SessionEndOutcome>(),
        "SessionsResponse": serializer_schema::<SessionsResponse>(),
        "SupervisorCommandReceipt": serializer_schema::<SupervisorCommandReceipt>(),
    });
    let checked_in: Value = serde_json::from_str(include_str!(
        "../../../../hub-web/test/fixtures/hub-wire.schema.json"
    ))
    .unwrap();
    if checked_in != generated {
        panic!("HUB_DOUBLE_SCHEMA {generated}");
    }
}
