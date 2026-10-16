//! Managed v2 reader intake. Legacy watches never decide managed routes.
use std::time::{Duration, Instant};

use cas_store::{SqliteVioletAdmissionStore, VioletAdmissionInput};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::coordination::external_work::{
    CALL_REMOTE_BUDGET, CONTRACT_SCHEMA, OperationContext, RemoteError, TOTAL_REMOTE_BUDGET,
};

/// Owned Cloud boundary. Implementations authenticate the registered actor;
/// the caller cannot choose its slot or route recipients.
pub(crate) trait ActivityDeliveryPort {
    fn claim(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError>;
    fn acknowledge(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError>;
}

// Violet performs managed sweep observation after an ordinary read. There is
// no standalone observe tool and a read receipt is not an observe acknowledgement.

#[derive(Debug, Deserialize)]
struct ReaderDelivery {
    id: String,
    occurrence_id: String,
    reader_slot_id: String,
    registration_id: String,
    route_kind: String,
    route_generation: i64,
    delivery_generation: i64,
    consumption: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct LeasedDelivery {
    delivery: ReaderDelivery,
    lease_token: String,
    target: ActivityTarget,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct ActivityTarget {
    resource_id: String,
    project_id: String,
    external_ref: SlackThread,
    message_ts: String,
    permalink: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct SlackThread {
    provider: String,
    workspace_id: String,
    channel_id: String,
    root_ts: String,
}

#[derive(Debug, Default)]
pub(crate) struct IntakeReport {
    pub admitted: usize,
    pub created: usize,
    pub errors: Vec<String>,
}

/// Runs only on bounded background jobs; it performs no v1 fallback.
pub(crate) fn poll_once(
    relay: &dyn ActivityDeliveryPort,
    store: &SqliteVioletAdmissionStore,
    context: &OperationContext,
    factory_session: &str,
    recipient: &str,
) -> IntakeReport {
    let mut report = IntakeReport::default();
    let started = Instant::now();
    let budget = || {
        TOTAL_REMOTE_BUDGET
            .saturating_sub(started.elapsed())
            .min(CALL_REMOTE_BUDGET)
    };
    let actor = json!({"registration_id": context.registration_id});
    let result = (|| -> Result<(), String> {
        let response = relay
            .claim(
                &json!({"schema": CONTRACT_SCHEMA, "actor": actor,
            "max": 25, "lease_secs": 60}),
                budget(),
            )
            .map_err(|error| error.message)?;
        require_completed(&response)?;
        let deliveries: Vec<LeasedDelivery> = serde_json::from_value(
            response
                .get("deliveries")
                .cloned()
                .ok_or("claim has no deliveries")?,
        )
        .map_err(|error| error.to_string())?;
        for leased in deliveries {
            match admission_input(&leased, context, factory_session, recipient)
                .and_then(|input| store.admit(&input).map_err(|error| error.to_string()))
            {
                Ok(receipt) => {
                    report.admitted += 1;
                    report.created += usize::from(receipt.created);
                }
                Err(error) => report.errors.push(error),
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.errors.push(error);
    }
    // Reconcile durable acks even after a failed/empty claim. Each pending row
    // was committed atomically with its queue entry and cursor before it appears.
    let settled = (|| -> Result<(), String> {
        let pending = store
            .pending_acknowledgements(&context.registration_id, &context.project_id)
            .map_err(|error| error.to_string())?;
        if pending.is_empty() {
            return Ok(());
        }
        if budget().is_zero() {
            return Err("remote budget exhausted; durable admissions await ack".into());
        }
        let acks: Vec<Value> = pending
            .iter()
            .map(|ack| {
                json!({
                    "delivery_id": ack.delivery_id, "lease_token": ack.lease_token,
                    "delivery_generation": ack.delivery_generation, "outcome": "admitted"
                })
            })
            .collect();
        let response = relay
            .acknowledge(
                &json!({"schema": CONTRACT_SCHEMA, "actor": actor, "acks": acks}),
                budget(),
            )
            .map_err(|error| error.message)?;
        require_completed(&response)?;
        let results = response["results"]
            .as_array()
            .ok_or("ack has no per-delivery results")?;
        for ack in &pending {
            let matching: Vec<_> = results
                .iter()
                .filter(|row| row["delivery_id"].as_str() == Some(ack.delivery_id.as_str()))
                .collect();
            if matching.len() != 1 {
                report.errors.push(format!(
                    "ack {} has missing or ambiguous result",
                    ack.delivery_id
                ));
                continue;
            }
            let row = matching[0];
            if row["code"] != "completed" || row["status"] != "admitted" {
                report.errors.push(format!(
                    "ack {} refused: {} {}",
                    ack.delivery_id, row["code"], row["status"]
                ));
                continue;
            }
            store
                .settle_acknowledgement(ack, &context.registration_id, &context.project_id)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = settled {
        report.errors.push(error);
    }
    report
}

fn require_completed(value: &Value) -> Result<(), String> {
    if value["schema"] != CONTRACT_SCHEMA || value["code"] != "completed" {
        return Err(format!(
            "managed intake refused: {} {}",
            value["code"], value["guidance"]
        ));
    }
    Ok(())
}

fn admission_input(
    leased: &LeasedDelivery,
    context: &OperationContext,
    session: &str,
    recipient: &str,
) -> Result<VioletAdmissionInput, String> {
    let delivery = &leased.delivery;
    let target = &leased.target;
    if delivery.registration_id != context.registration_id
        || target.project_id != context.project_id
        || target.external_ref.provider != "slack"
        || delivery.consumption != "unread"
        || delivery.status != "leased"
        || !matches!(delivery.route_kind.as_str(), "owner" | "unclaimed_fanout")
        || delivery.delivery_generation < 1
        || delivery.route_generation < 0
    {
        return Err(
            "delivery registration/project/route does not match this authenticated reader".into(),
        );
    }
    let prompt = managed_activity_envelope(leased);
    Ok(VioletAdmissionInput {
        delivery_id: delivery.id.clone(),
        occurrence_id: delivery.occurrence_id.clone(),
        reader_slot_id: delivery.reader_slot_id.clone(),
        registration_id: delivery.registration_id.clone(),
        delivery_generation: delivery.delivery_generation,
        route_generation: delivery.route_generation,
        project_id: target.project_id.clone(),
        target_json: serde_json::to_string(target).map_err(|e| e.to_string())?,
        workspace_id: target.external_ref.workspace_id.clone(),
        channel_id: target.external_ref.channel_id.clone(),
        message_ts: target.message_ts.clone(),
        factory_session: session.into(),
        recipient: recipient.into(),
        prompt,
        lease_token: leased.lease_token.clone(),
    })
}

fn managed_activity_envelope(leased: &LeasedDelivery) -> String {
    use crate::prompt_revalidation::xml_attribute_value as attr;
    let target = &leased.target;
    let delivery = &leased.delivery;
    format!(
        "<cas-violet-activity v=\"2\" channel=\"{}\" channel_name=\"{}\" kind=\"{}\" \
        occurrence=\"{}\" reader=\"{}\" route_generation=\"{}\" delivery=\"{}\">\n\
        Slack occurrence message_ts={} thread_ts={} workspace={}.\n\
        Read the verified thread through Violet. This route is a notification, never ownership or reply authority. \
        Obtain a current external-work claim before work; managed replies require the current grant generation.\n\
        </cas-violet-activity>",
        attr(&target.external_ref.channel_id),
        attr(&target.external_ref.channel_id),
        attr(&delivery.route_kind),
        attr(&delivery.occurrence_id),
        attr(&delivery.reader_slot_id),
        delivery.route_generation,
        attr(&delivery.id),
        attr(&target.message_ts),
        attr(&target.external_ref.root_ts),
        attr(&target.external_ref.workspace_id)
    )
}

#[cfg(test)]
#[path = "violet_intake_tests.rs"]
mod tests;

#[path = "violet_recovery.rs"]
pub(crate) mod recovery;

#[path = "violet_binding_evidence.rs"]
pub(crate) mod activation;
