//! Recover a rejected Cloud login without opening a browser or replaying uncertain writes.
use super::CloudConfig;
use serde::Serialize;

pub(crate) const EXPIRED_MESSAGE: &str = "Cloud session expired, run cas login";

/// The direct-token login path shared by `cas login` and bounded 401 recovery.
pub(crate) fn verify_and_store_token(endpoint: &str, token: &str) -> anyhow::Result<()> {
    let response = ureq::get(&format!("{endpoint}/api/sync/status"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(30))
        .call();
    match response {
        Ok(response) if response.status() == 200 => {}
        Ok(response) => anyhow::bail!("Invalid token or server error: {}", response.status()),
        Err(ureq::Error::Status(401, _)) => anyhow::bail!("Invalid API token"),
        Err(error) => anyhow::bail!("Failed to connect to Cassy Cloud: {error}"),
    }
    super::store_login_credentials(endpoint, token, None, None)?;
    Ok(())
}

fn endpoint(request: &ureq::Request) -> Option<String> {
    let mut url = url::Url::parse(request.url()).ok()?;
    let prefix = url.path().split_once("/api/")?.0.to_owned();
    url.set_path(&prefix);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.as_str().trim_end_matches('/').to_owned())
}

fn recover_login(endpoint: &str, rejected: &str) -> Option<String> {
    // Recheck inside the lock: another caller may just have restored the login.
    static RECOVERY: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = RECOVERY.lock().unwrap_or_else(|error| error.into_inner());
    for config in [CloudConfig::load().ok(), CloudConfig::load_user().ok()]
        .into_iter()
        .flatten()
    {
        if config.endpoint.trim_end_matches('/') == endpoint
            && let Some(token) = config
                .token
                .as_deref()
                .filter(|token| !token.is_empty() && *token != rejected)
        {
            super::store_login_credentials(
                endpoint,
                token,
                config.email.as_deref(),
                config.plan.as_deref(),
            )
            .ok()?;
            return Some(token.to_owned());
        }
    }
    // An environment credential belongs to its configured endpoint; never send
    // the machine's production key to an unrelated custom Cloud origin.
    if super::default_endpoint().trim_end_matches('/') != endpoint {
        return None;
    }
    let token = cas_core::env_overlay::var("CAS_CLOUD_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())?;
    verify_and_store_token(endpoint, &token).ok()?;
    Some(token)
}

fn expired() -> ureq::Error {
    let body =
        serde_json::json!({"code":"cloud_session_expired","error":EXPIRED_MESSAGE}).to_string();
    ureq::Error::Status(
        401,
        ureq::Response::new(401, "Unauthorized", &body).expect("static authorization response"),
    )
}

// Cloud authentication returns this protocol error for invalid legacy keys and
// expired/revoked read-only keys (Cloud lib/auth.ts). Other 401s belong to the
// route, and must retain their original cause without attempting login recovery.
fn classify_key_failure(
    result: Result<ureq::Response, ureq::Error>,
) -> (bool, Result<ureq::Response, ureq::Error>) {
    let response = match result {
        Err(ureq::Error::Status(401, response)) => response,
        other => return (false, other),
    };
    let status_text = response.status_text().to_owned();
    let body = match response.into_string() {
        Ok(body) => body,
        Err(error) => return (false, Err(error.into())),
    };
    let key_failure = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .is_some_and(|value| value["error"].as_str() == Some("Invalid API key"));
    let response = ureq::Response::new(401, &status_text, &body)
        .expect("valid buffered Cloud authorization response");
    (key_failure, Err(ureq::Error::Status(401, response)))
}

fn send(
    request: ureq::Request,
    mut dispatch: impl FnMut(ureq::Request) -> Result<ureq::Response, ureq::Error>,
) -> Result<ureq::Response, ureq::Error> {
    let (key_failure, result) = classify_key_failure(dispatch(request.clone()));
    if !key_failure {
        return result;
    }
    let Some(original) = request
        .header("Authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return result;
    };
    let Some(endpoint) = endpoint(&request) else {
        return result;
    };
    let Some(token) = recover_login(&endpoint, original) else {
        return Err(expired());
    };
    // The API key was rejected. Clone the request and retain
    // its body and operation IDs; transport failures and other statuses never retry here.
    let (key_failure, result) = classify_key_failure(dispatch(
        request.set("Authorization", &format!("Bearer {token}")),
    ));
    if key_failure { Err(expired()) } else { result }
}

pub(crate) trait CloudRequestExt {
    fn cloud_call(self) -> Result<ureq::Response, ureq::Error>;
    fn cloud_send_json(self, body: impl Serialize) -> Result<ureq::Response, ureq::Error>;
}

impl CloudRequestExt for ureq::Request {
    fn cloud_call(self) -> Result<ureq::Response, ureq::Error> {
        send(self, |request| request.call())
    }
    fn cloud_send_json(self, body: impl Serialize) -> Result<ureq::Response, ureq::Error> {
        send(self, |request| request.send_json(&body))
    }
}
