use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde_json::{Value, json};

use super::{EndSessionReceipt, LeaseRefusal, SessionsResponse};
use crate::hub::{HubSession, LeaseSummary, MachineEvent};
use crate::ui::factory::{SessionEndOutcome, supervisor_commands::SupervisorCommandReceipt};

fn serializer_schema<Wire: JsonSchema>() -> Schema {
    let mut schema = SchemaSettings::draft2020_12()
        .for_serialize()
        .with_transform(RecursiveTransform(|schema: &mut Schema| {
            schema.remove("description");
        }))
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
