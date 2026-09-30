//! Mock only the HTTP boundary; sync dispatch, serialization and stores stay real.

use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, path_regex, query_param};
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

    async fn mock_project_aliases(&self) {
        Mock::given(method("GET"))
            .and(path("/api/account/projects"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"projects": []})))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    pub async fn mock_pull_with_data(
        &self,
        project_id: &str,
        entries: Vec<Value>,
        tasks: Vec<Value>,
    ) {
        self.mock_project_aliases().await;
        Mock::given(method("GET"))
            .and(path("/api/sync/pull"))
            .and(query_param("project_id", project_id))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "entries": entries,
                "tasks": tasks,
                "rules": [],
                "skills": [],
                "pulled_at": "2026-09-30T00:00:00Z"
            })))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    pub async fn mock_pull_refusal(&self, project_id: &str, status: u16) {
        self.mock_project_aliases().await;
        Mock::given(method("GET"))
            .and(path("/api/sync/pull"))
            .and(query_param("project_id", project_id))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": "refused"})))
            .expect(1)
            .mount(&self.server)
            .await;
    }

    pub async fn expect_no_api_requests(&self) {
        Mock::given(path_regex("^/api/.*"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
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
