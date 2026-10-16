//! Bounded retries for idempotent cleanup requests. Call only off-loop.
use std::time::{Duration, Instant};

pub(crate) const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
const ATTEMPTS: u32 = 3;

/// Both HTTP Retry-After forms. Invalid values fall back to exponential backoff.
pub(crate) fn retry_after(response: &ureq::Response) -> Option<Duration> {
    let value = response.header("Retry-After")?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}

/// A single budget covers requests and sleeps. A server delay outside that
/// budget is returned without an early retry; durable callers schedule it later.
pub(crate) fn call(
    budget: Duration,
    mut request: impl FnMut(Duration) -> Result<ureq::Response, ureq::Error>,
) -> Result<ureq::Response, ureq::Error> {
    let deadline = Instant::now() + budget.min(CLEANUP_BUDGET);
    for attempt in 0..ATTEMPTS {
        let response = request(deadline.saturating_duration_since(Instant::now()));
        let Err(ureq::Error::Status(429, ref refused)) = response else {
            return response;
        };
        let delay = retry_after(refused)
            .unwrap_or_else(|| Duration::from_millis(200 << attempt))
            .max(Duration::from_millis(10));
        if attempt + 1 == ATTEMPTS || delay >= deadline.saturating_duration_since(Instant::now()) {
            return response;
        }
        std::thread::sleep(delay);
    }
    unreachable!("last attempt returns")
}

pub(crate) struct Failure {
    pub cause: String,
    pub retry_after: Option<Duration>,
    pub status: Option<u16>,
}

impl Failure {
    pub fn from_http(what: &str, error: ureq::Error) -> Self {
        match error {
            ureq::Error::Status(code, response) => {
                let retry_after = (code == 429).then(|| retry_after(&response)).flatten();
                let cause = if code == 429 {
                    format!("{what}: rate limited (429)")
                } else {
                    format!(
                        "{what} refused ({code}): {}",
                        response.into_string().unwrap_or_default()
                    )
                };
                Self {
                    cause,
                    retry_after,
                    status: Some(code),
                }
            }
            ureq::Error::Transport(error) => Self {
                cause: format!("{what}: network error: {error}"),
                retry_after: None,
                status: None,
            },
        }
    }
}
