//! Mock only the HTTP boundary; sync dispatch, serialization and stores stay real.

use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct CloudMockServer {
    pub server: MockServer,
    pub endpoint: String,
}

impl CloudMockServer {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let endpoint = server.uri();
        Self { server, endpoint }
    }

    pub async fn mock_push_success(
        &self,
        entries: usize,
        tasks: usize,
        rules: usize,
        skills: usize,
    ) {
        Mock::given(method("POST"))
            .and(path("/api/sync/push"))
            .and(header("Authorization", "Bearer test-token"))
            .and(header("Content-Encoding", "gzip"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "entries": { "inserted": entries, "updated": 0 },
                "tasks": { "inserted": tasks, "updated": 0 },
                "rules": { "inserted": rules, "updated": 0 },
                "skills": { "inserted": skills, "updated": 0 }
            })))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    /// Decode the actual uploaded envelope, including the adapter's gzip encoding.
    pub async fn pushed_payload(&self) -> Value {
        let requests = self.server.received_requests().await.unwrap();
        let pushes: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path() == "/api/sync/push")
            .collect();
        assert_eq!(pushes.len(), 1, "exactly one uploaded envelope");
        let decoded = flate2::read::GzDecoder::new(pushes[0].body.as_slice());
        serde_json::from_reader(decoded).expect("uploaded gzip JSON")
    }
}
