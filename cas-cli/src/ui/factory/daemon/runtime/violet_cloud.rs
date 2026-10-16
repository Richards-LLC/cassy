//! Owned HTTP adapter for managed reader delivery and recovery surfaces.
use super::*;
use crate::coordination::external_work::{CloudCoordinationPort, HttpCloudCoordinationPort};

pub(crate) trait ActivityRecoveryPort: CloudCoordinationPort {
    fn recover(
        &self,
        context: &OperationContext,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, RemoteError>;
    fn delivery(
        &self,
        context: &OperationContext,
        id: &str,
        timeout: Duration,
    ) -> Result<Value, RemoteError>;
    fn activation(
        &self,
        context: &OperationContext,
        timeout: Duration,
    ) -> Result<Value, RemoteError>;
}

impl ActivityDeliveryPort for HttpCloudCoordinationPort {
    fn claim(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        self.post("/api/violet/activity/claim", request, timeout)
    }
    fn acknowledge(&self, request: &Value, timeout: Duration) -> Result<Value, RemoteError> {
        self.post("/api/violet/activity/ack", request, timeout)
    }
    fn record_consumption(
        &self,
        delivery: &str,
        request: &Value,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        uuid::Uuid::parse_str(delivery)
            .map_err(|_| RemoteError::unavailable("invalid delivery UUID"))?;
        self.post(
            &format!("/api/coordination/activity/deliveries/{delivery}/receipts"),
            request,
            timeout,
        )
    }
}

impl ActivityRecoveryPort for HttpCloudCoordinationPort {
    fn recover(
        &self,
        context: &OperationContext,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        let mut query = vec![
            ("registration_id", context.registration_id.as_str()),
            ("limit", "25"),
        ];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor));
        }
        self.get("/api/coordination/activity/deliveries", &query, timeout)
    }
    fn delivery(
        &self,
        context: &OperationContext,
        id: &str,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        uuid::Uuid::parse_str(id).map_err(|_| RemoteError::unavailable("invalid delivery UUID"))?;
        self.get(
            &format!("/api/coordination/activity/deliveries/{id}"),
            &[("registration_id", context.registration_id.as_str())],
            timeout,
        )
    }
    fn activation(
        &self,
        context: &OperationContext,
        timeout: Duration,
    ) -> Result<Value, RemoteError> {
        self.get(
            "/api/coordination/bindings/activation",
            &[("registration_id", context.registration_id.as_str())],
            timeout,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cas_c423_http_reader_adapter_authenticates_gets_and_preserves_refusals() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", server.server_addr());
        let serving = std::thread::spawn(move || {
            for (path, status, body) in [
                (
                    "/api/coordination/activity/deliveries?registration_id=reg%2Fsuccessor&limit=25&cursor=previous",
                    200,
                    json!({"schema":CONTRACT_SCHEMA,"code":"completed","admissions":[]}),
                ),
                (
                    "/api/coordination/activity/deliveries/0000de1b-1111-4111-8111-00000000de1b?registration_id=reg%2Fsuccessor",
                    404,
                    json!({"schema":CONTRACT_SCHEMA,"code":"not_found"}),
                ),
                (
                    "/api/coordination/bindings/activation?registration_id=reg%2Fsuccessor",
                    403,
                    json!({"schema":CONTRACT_SCHEMA,"code":"coordination_scope_required"}),
                ),
            ] {
                let request = server
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .expect("reader HTTP request");
                assert_eq!(request.method(), &tiny_http::Method::Get);
                assert_eq!(request.url(), path);
                assert!(
                    request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("Authorization")
                            && h.value.as_str() == "Bearer local-fixture-token")
                );
                request
                    .respond(
                        tiny_http::Response::from_string(body.to_string()).with_status_code(status),
                    )
                    .unwrap();
            }
        });
        let port = HttpCloudCoordinationPort::new(endpoint, "local-fixture-token");
        let context = OperationContext {
            project_id: "project".into(),
            registration_id: "reg/successor".into(),
        };
        assert_eq!(
            port.recover(&context, Some("previous"), CALL_REMOTE_BUDGET)
                .unwrap()["admissions"],
            json!([])
        );
        assert_eq!(
            port.delivery(
                &context,
                "0000de1b-1111-4111-8111-00000000de1b",
                CALL_REMOTE_BUDGET
            )
            .unwrap()["code"],
            "not_found"
        );
        assert_eq!(
            port.activation(&context, CALL_REMOTE_BUDGET).unwrap()["code"],
            "coordination_scope_required"
        );
        serving.join().unwrap();
        assert!(
            port.delivery(&context, "../other", CALL_REMOTE_BUDGET)
                .is_err()
        );
    }
}
