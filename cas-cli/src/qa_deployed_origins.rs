//! cas-f426: `qa.deployed_origins` entries, exact or per-deployment patterns.
//!
//! An entry is an exact origin (`https://app.example.com`) or a pattern
//! whose leftmost host label holds one `*`, such as Vercel's per-deployment
//! hosts `https://app-*-team.vercel.app`. The `*` matches one or more
//! letters, digits or hyphens inside that label, never a dot, so a pattern
//! cannot reach another domain. A pattern still needs literal text in its
//! wildcard label and at least three labels: `https://*.vercel.app` would
//! admit every Vercel project and is refused.

use super::{is_local_origin, url_origin};

/// The Vercel shape shown in refusals; projects substitute their own names.
pub const VERCEL_PATTERN_EXAMPLE: &str = "https://<project>-*-<team>.vercel.app";

/// Validate and normalise one configured entry.
pub fn normalize(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if !raw.contains('*') {
        let origin = url_origin(raw).ok_or_else(|| format!("{raw:?} is not an http(s) origin"))?;
        if is_local_origin(&origin) {
            return Err(format!(
                "{origin} is local; a local build needs no deployed origin"
            ));
        }
        return Ok(origin);
    }
    let lowered = raw.to_ascii_lowercase();
    let (scheme, rest) = lowered
        .split_once("://")
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .ok_or_else(|| format!("{raw:?} is not an http(s) origin pattern"))?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.contains(['/', '?', '#', '@']) {
        return Err(format!(
            "{raw:?} is not an origin pattern: give scheme and host only, like {VERCEL_PATTERN_EXAMPLE}"
        ));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    let labels: Vec<&str> = host.split('.').collect();
    let wildcard_ok = labels.len() >= 3
        && labels[0].matches('*').count() == 1
        && labels[0] != "*"
        && labels[1..].iter().all(|label| !label.contains('*'))
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '*')
        });
    if !wildcard_ok || host.ends_with(".localhost") {
        return Err(format!(
            "{raw:?}: a pattern takes one `*` inside the leftmost host label, beside literal text, on a remote host of three or more labels, like {VERCEL_PATTERN_EXAMPLE}"
        ));
    }
    let port = match port {
        None => None,
        Some(port) => {
            let number: u16 = port
                .parse()
                .map_err(|_| format!("{raw:?} has an invalid port"))?;
            let default = if scheme == "https" { 443 } else { 80 };
            (number != default).then_some(number)
        }
    };
    Ok(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Whether the normalised `origin` is admitted by any configured entry.
pub fn allows(configured: &[String], origin: &str) -> bool {
    let Some(origin) = url_origin(origin) else {
        return false;
    };
    configured
        .iter()
        .filter_map(|entry| normalize(entry).ok())
        .any(|entry| matches_entry(&entry, &origin))
}

fn matches_entry(entry: &str, origin: &str) -> bool {
    let Some((prefix, suffix)) = entry.split_once('*') else {
        return entry == origin;
    };
    origin.len() > prefix.len() + suffix.len()
        && origin.starts_with(prefix)
        && origin.ends_with(suffix)
        && origin[prefix.len()..origin.len() - suffix.len()]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

/// Host of an entry, with the `*` kept, for host guards.
pub fn host(entry: &str) -> Option<String> {
    let normalized = normalize(entry).ok()?;
    let authority = normalized.split_once("://")?.1;
    Some(
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
            .to_string(),
    )
}

/// How to satisfy the gate from a deployment when no local build can sign in.
pub fn setup_hint(configured: &[String]) -> String {
    let declare =
        "declare `deployed` in bundle.json (origin, reason, deployed_sha, deployment_proof)";
    if configured.is_empty() {
        format!(
            "No local build that can sign in? Walk the deployment instead: a supervisor runs `cas config set qa.deployed_origins <origin>[,<origin>]` in this project (qa.deployed_origins is empty; per-deployment hosts take a pattern such as {VERCEL_PATTERN_EXAMPLE}), then {declare}"
        )
    } else {
        format!(
            "No local build that can sign in? Walk a configured deployment (qa.deployed_origins: {}) and {declare}",
            configured.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_deployment_patterns_admit_one_label_only_cas_f426() {
        let configured =
            vec![normalize("https://pulse-card-frontend-*-richards-llc.vercel.app/").unwrap()];
        assert_eq!(
            configured[0],
            "https://pulse-card-frontend-*-richards-llc.vercel.app"
        );
        assert!(allows(
            &configured,
            "https://pulse-card-frontend-bzrfqei28-richards-llc.vercel.app/admin"
        ));
        for refused in [
            "https://evil.com/x.pulse-card-frontend-a-richards-llc.vercel.app",
            "https://pulse-card-frontend-a.b-richards-llc.vercel.app",
            "https://pulse-card-frontend--richards-llc.vercel.app",
            "http://pulse-card-frontend-a-richards-llc.vercel.app",
            "https://other-a-richards-llc.vercel.app",
        ] {
            assert!(!allows(&configured, refused), "{refused}");
        }
        for invalid in [
            "https://*.vercel.app",
            "https://app.*-team.vercel.app",
            "https://a-*-*.team.vercel.app",
            "https://app-*-team.vercel.app/path",
            "ftp://app-*-team.vercel.app",
            "https://app-*-team.qa.localhost",
        ] {
            assert!(normalize(invalid).is_err(), "{invalid}");
        }
    }
}
