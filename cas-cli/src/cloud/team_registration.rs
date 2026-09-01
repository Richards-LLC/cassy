//! Explicit team↔project registration for `cas cloud sync` (cas-c117).
//!
//! # Why this module exists
//!
//! The server only learns that a project belongs to a team as a *side effect*
//! of `POST /api/teams/{teamId}/sync/push`: the route upserts the project row
//! (`INSERT … ON CONFLICT DO NOTHING`) from the `project_canonical_id` field of
//! the push payload. The client, however, only issues that POST when the team
//! sync queue has rows to send — [`CloudSyncer::push_team`] returns early with
//! an empty [`SyncResult`] when `pending_for_team` is empty
//! (`cloud/syncer/team_push.rs`).
//!
//! Team rows are enqueued at *write* time (`store/syncing_*.rs`), so a clean
//! install, a fresh clone, or any machine that simply hasn't written anything
//! since the team was configured has an empty team queue. `cas cloud sync` then
//! prints "✓ Push complete / ✓ Pull complete", exits 0, and the project is
//! still unknown to the team — after which `cas cloud team-memories` reports
//! "This project hasn't been synced to the team yet. Run `cas cloud sync`",
//! which is exactly what the user just did. Ben hit this on a macOS
//! clean install (cas-c117, EPIC cas-e0d9).
//!
//! # What this module does
//!
//! [`TeamRegistration::ensure`] makes registration explicit and *verified*:
//!
//! 1. `GET /api/teams/{teamId}/projects` — is the canonical id already listed?
//! 2. If not, `POST /api/teams/{teamId}/sync/push` carrying only
//!    `project_canonical_id` (+ `git_remote`) and an empty `entries` array —
//!    the same request shape a normal push uses, minus the rows. That is the
//!    server's registration path.
//! 3. `GET /api/teams/{teamId}/projects` again — the registration is only
//!    "done" when the server itself lists the project.
//!
//! Every failure is returned as a [`RegistrationFailure`] carrying both a
//! human reason and the exact HTTP interaction that failed, so the caller can
//! exit non-zero with a real explanation instead of a green checkmark.

use std::fmt;
use std::time::Duration;

use crate::cloud::{CloudSyncer, TeamProjectsResponse};

/// Timeout for both the lookup and the registration request. Matches the
/// 30 s used by the other interactive team endpoints in `cli/cloud.rs`.
pub const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Successful outcome of [`TeamRegistration::ensure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The server already listed the project for this team — no write needed.
    AlreadyRegistered { project_uuid: String },
    /// The project was missing and the server accepted + confirmed it.
    Registered { project_uuid: String },
    /// The registration push resolved to an existing project under a different
    /// canonical id. The caller must persist this id before the same sync run
    /// performs its regular team push and pull.
    AdoptedExisting {
        project_uuid: String,
        canonical_id: String,
    },
}

impl RegistrationOutcome {
    /// Server-side UUID of the team project.
    pub fn project_uuid(&self) -> &str {
        match self {
            Self::AlreadyRegistered { project_uuid }
            | Self::Registered { project_uuid }
            | Self::AdoptedExisting { project_uuid, .. } => project_uuid,
        }
    }

    /// `true` when this run created the registration (worth telling the user).
    pub fn newly_registered(&self) -> bool {
        matches!(self, Self::Registered { .. })
    }

    /// The server-resolved id that must be pinned for future sync operations.
    pub fn adopted_canonical_id(&self) -> Option<&str> {
        match self {
            Self::AdoptedExisting { canonical_id, .. } => Some(canonical_id),
            _ => None,
        }
    }
}

/// A registration attempt that did not end with the server listing the
/// project. Carries an operator-facing `reason` and the precise
/// `interaction` (method, URL, status, body excerpt) for escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationFailure {
    /// Human, actionable statement of what went wrong.
    pub reason: String,
    /// The exact request/response that failed — for bug reports against the
    /// cloud server, which lives in a separate repository.
    pub interaction: String,
}

impl fmt::Display for RegistrationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\n  Failing interaction: {}",
            self.reason, self.interaction
        )
    }
}

impl std::error::Error for RegistrationFailure {}

/// Trim a server body for inclusion in an error message. Bodies can be large
/// HTML error pages; the first 300 characters are enough to identify them.
fn body_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty body>".to_string();
    }
    if trimmed.chars().count() <= 300 {
        return trimmed.replace('\n', " ");
    }
    let head: String = trimmed.chars().take(300).collect();
    format!("{}…", head.replace('\n', " "))
}

/// One project↔team registration check, parameterized so integration tests can
/// point it at a wiremock server.
pub struct TeamRegistration<'a> {
    endpoint: &'a str,
    token: &'a str,
    team_id: &'a str,
    canonical_id: &'a str,
    git_remote: Option<&'a str>,
    timeout: Duration,
}

impl<'a> TeamRegistration<'a> {
    /// Build a registration for `canonical_id` against `team_id`.
    pub fn new(endpoint: &'a str, token: &'a str, team_id: &'a str, canonical_id: &'a str) -> Self {
        Self {
            endpoint,
            token,
            team_id,
            canonical_id,
            git_remote: None,
            timeout: REGISTRATION_TIMEOUT,
        }
    }

    /// Attach the normalized git remote so the server's project resolver can
    /// map this machine onto the team's existing bucket (cas-8ca5 contract §5)
    /// instead of creating a second one.
    pub fn with_git_remote(mut self, git_remote: Option<&'a str>) -> Self {
        self.git_remote = git_remote;
        self
    }

    /// Override the HTTP timeout (tests use a short one).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Verify that the project is already registered with the team without
    /// creating a registration as a side effect. Reassignment uses this
    /// read-only path so an invalid destination is rejected before the local
    /// task or its sync queue changes.
    pub fn verify_registered(&self) -> Result<(), RegistrationFailure> {
        if self.lookup(self.canonical_id)?.is_some() {
            return Ok(());
        }

        Err(RegistrationFailure {
            reason: format!(
                "Project '{}' is not registered with team {} on {}.",
                self.canonical_id, self.team_id, self.endpoint
            ),
            interaction: format!(
                "GET {} -> 200 but no project with canonical_id \"{}\"",
                self.projects_url(),
                self.canonical_id
            ),
        })
    }

    fn projects_url(&self) -> String {
        format!("{}/api/teams/{}/projects", self.endpoint, self.team_id)
    }

    fn push_url(&self) -> String {
        format!("{}/api/teams/{}/sync/push", self.endpoint, self.team_id)
    }

    /// Verify (and if necessary create) the project↔team registration.
    ///
    /// Returns `Ok` only when the *server* lists the project — a 2xx on the
    /// registration write is not treated as proof on its own.
    pub fn ensure(&self) -> Result<RegistrationOutcome, RegistrationFailure> {
        if let Some(project_uuid) = self.lookup(self.canonical_id)? {
            return Ok(RegistrationOutcome::AlreadyRegistered { project_uuid });
        }

        let (register_status, resolved_id) = self.register()?;
        let expected_id = resolved_id.as_deref().unwrap_or(self.canonical_id);

        match self.lookup(expected_id)? {
            Some(project_uuid) if expected_id == self.canonical_id => {
                Ok(RegistrationOutcome::Registered { project_uuid })
            }
            Some(project_uuid) => Ok(RegistrationOutcome::AdoptedExisting {
                project_uuid,
                canonical_id: expected_id.to_string(),
            }),
            None => Err(RegistrationFailure {
                reason: if expected_id == self.canonical_id {
                    format!(
                        "Project '{}' is still not registered with team {} on {} after the \
                         registration request succeeded. The server accepted the write but does \
                         not list the project, so team memories and team pushes for this project \
                         cannot work. This is a server-side defect — report the interaction below.",
                        self.canonical_id, self.team_id, self.endpoint
                    )
                } else {
                    format!(
                        "The server resolved project '{}' to '{}' during registration, but \
                         team {} on {} did not list the resolved project afterward. The client \
                         will not adopt '{}' until that verification succeeds.",
                        self.canonical_id, expected_id, self.team_id, self.endpoint, expected_id,
                    )
                },
                interaction: format!(
                    "POST {} (body: {{\"entries\":[],\"project_canonical_id\":\"{}\"{}}}) -> {} \
                     (resolved canonical_id: {}); \
                     GET {} -> 200 but no project with canonical_id \"{}\"",
                    self.push_url(),
                    self.canonical_id,
                    match self.git_remote {
                        Some(remote) => format!(",\"git_remote\":\"{remote}\""),
                        None => String::new(),
                    },
                    register_status,
                    resolved_id.as_deref().unwrap_or("<absent>"),
                    self.projects_url(),
                    expected_id,
                ),
            }),
        }
    }

    /// `GET /api/teams/{teamId}/projects` → the project UUID when the team
    /// already knows `canonical_id`.
    fn lookup(&self, canonical_id: &str) -> Result<Option<String>, RegistrationFailure> {
        let url = self.projects_url();
        let response = ureq::get(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .timeout(self.timeout)
            .call();

        let resp = match response {
            Ok(resp) => resp,
            Err(ureq::Error::Status(401, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(RegistrationFailure {
                    reason: "Cloud session expired — could not confirm this project is \
                             registered with your team. Run `cas login` to re-authenticate."
                        .to_string(),
                    interaction: format!("GET {url} -> 401 {}", body_excerpt(&body)),
                });
            }
            Err(ureq::Error::Status(403, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(RegistrationFailure {
                    reason: format!(
                        "Your account is not a member of team {} on {} — the project cannot \
                         be registered. Run `cas cloud team set <uuid>` with a team you belong to.",
                        self.team_id, self.endpoint
                    ),
                    interaction: format!("GET {url} -> 403 {}", body_excerpt(&body)),
                });
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                return Err(RegistrationFailure {
                    reason: format!(
                        "Could not confirm this project is registered with team {}: the server \
                         returned HTTP {code}.",
                        self.team_id
                    ),
                    interaction: format!("GET {url} -> {code} {}", body_excerpt(&body)),
                });
            }
            Err(ureq::Error::Transport(e)) => {
                return Err(RegistrationFailure {
                    reason: format!(
                        "Could not reach {} to confirm this project is registered with your \
                         team: {e}",
                        self.endpoint
                    ),
                    interaction: format!("GET {url} -> transport error: {e}"),
                });
            }
        };

        let body = resp.into_string().unwrap_or_default();
        let parsed: TeamProjectsResponse =
            serde_json::from_str(&body).map_err(|e| RegistrationFailure {
                reason: format!(
                    "Could not confirm this project is registered with team {}: the server's \
                     project list could not be parsed ({e}).",
                    self.team_id
                ),
                interaction: format!("GET {url} -> 200 {}", body_excerpt(&body)),
            })?;

        Ok(parsed
            .projects
            .into_iter()
            .find(|p| p.canonical_id == canonical_id)
            .map(|p| p.id))
    }

    /// `POST /api/teams/{teamId}/sync/push` with no rows — the server's
    /// project-registration path. Returns its status plus the canonical id the
    /// server resolved the push into, if provided in the response body.
    fn register(&self) -> Result<(u16, Option<String>), RegistrationFailure> {
        let url = self.push_url();

        let mut payload = serde_json::Map::new();
        // An empty `entries` array keeps the request shape identical to a
        // normal team push (which the server already accepts) while writing
        // no rows — only the project registration.
        payload.insert("entries".to_string(), serde_json::json!([]));
        payload.insert(
            "project_canonical_id".to_string(),
            serde_json::json!(self.canonical_id),
        );
        if let Some(remote) = self.git_remote {
            payload.insert("git_remote".to_string(), serde_json::json!(remote));
        }
        CloudSyncer::insert_client_version(&mut payload);

        let json_bytes = serde_json::to_vec(&payload).map_err(|e| RegistrationFailure {
            reason: format!("Could not build the team registration request: {e}"),
            interaction: format!("POST {url} (payload serialization failed)"),
        })?;
        let compressed = CloudSyncer::gzip_json(&json_bytes).map_err(|e| RegistrationFailure {
            reason: format!("Could not compress the team registration request: {e}"),
            interaction: format!("POST {url} (gzip failed)"),
        })?;

        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "gzip")
            .timeout(self.timeout)
            .send_bytes(&compressed);

        match response {
            Ok(resp) => {
                let status = resp.status();
                if (200..300).contains(&status) {
                    let body = resp.into_string().unwrap_or_default();
                    let resolved_id = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("canonical_id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::trim)
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                        });
                    return Ok((status, resolved_id));
                }
                let body = resp.into_string().unwrap_or_default();
                Err(self.register_failure(&url, status, &body))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(self.register_failure(&url, code, &body))
            }
            Err(ureq::Error::Transport(e)) => Err(RegistrationFailure {
                reason: format!(
                    "Could not reach {} to register this project with your team: {e}",
                    self.endpoint
                ),
                interaction: format!("POST {url} -> transport error: {e}"),
            }),
        }
    }

    fn register_failure(&self, url: &str, status: u16, body: &str) -> RegistrationFailure {
        let reason = match status {
            401 => "Cloud session expired — could not register this project with your team. \
                    Run `cas login` to re-authenticate."
                .to_string(),
            403 => format!(
                "Your account is not allowed to register projects for team {} on {}.",
                self.team_id, self.endpoint
            ),
            _ => format!(
                "Could not register project '{}' with team {} on {}: the server returned \
                 HTTP {status}.",
                self.canonical_id, self.team_id, self.endpoint
            ),
        };
        RegistrationFailure {
            reason,
            interaction: format!(
                "POST {url} (body: {{\"entries\":[],\"project_canonical_id\":\"{}\"}}) -> \
                 {status} {}",
                self.canonical_id,
                body_excerpt(body)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_excerpt_reports_empty_bodies_explicitly() {
        assert_eq!(body_excerpt("   \n "), "<empty body>");
    }

    #[test]
    fn body_excerpt_flattens_and_truncates() {
        let long = "x".repeat(500);
        let out = body_excerpt(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 301);

        assert_eq!(body_excerpt("a\nb"), "a b");
    }

    #[test]
    fn failure_display_includes_reason_and_interaction() {
        let failure = RegistrationFailure {
            reason: "nope".to_string(),
            interaction: "GET /x -> 500".to_string(),
        };
        let rendered = failure.to_string();
        assert!(rendered.contains("nope"));
        assert!(rendered.contains("GET /x -> 500"));
    }

    #[test]
    fn outcome_reports_uuid_and_novelty() {
        let already = RegistrationOutcome::AlreadyRegistered {
            project_uuid: "p-1".to_string(),
        };
        assert_eq!(already.project_uuid(), "p-1");
        assert!(!already.newly_registered());

        let fresh = RegistrationOutcome::Registered {
            project_uuid: "p-2".to_string(),
        };
        assert_eq!(fresh.project_uuid(), "p-2");
        assert!(fresh.newly_registered());
        assert_eq!(fresh.adopted_canonical_id(), None);

        let adopted = RegistrationOutcome::AdoptedExisting {
            project_uuid: "p-3".to_string(),
            canonical_id: "legacy-slug".to_string(),
        };
        assert_eq!(adopted.project_uuid(), "p-3");
        assert!(!adopted.newly_registered());
        assert_eq!(adopted.adopted_canonical_id(), Some("legacy-slug"));
    }
}
