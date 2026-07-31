//! Stable, credential-free identities for untrusted names crossing public boundaries.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

const UPSTREAM_PREFIX: &str = "upstream-";
const TOOL_PREFIX: &str = "tool-";

/// Resolution of one public upstream identifier against a complete config.
///
/// Raw names are returned only to the internal caller and must never be
/// rendered in public output or errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicUpstreamIdResolution {
    Found {
        raw_name: String,
        public_name: String,
    },
    NotFound,
    Ambiguous,
}

/// Project one configured upstream name into a bounded public identity.
///
/// Simple operator names remain readable. Names that could carry paths, URLs,
/// controls, Markdown, or secret-shaped text are cryptographically
/// pseudonymized.
pub fn public_upstream_id(name: &str) -> String {
    public_id(name, UPSTREAM_PREFIX, 64, false)
}

/// Project one upstream tool name into a bounded public identity.
pub fn public_tool_id(name: &str) -> String {
    public_id(name, TOOL_PREFIX, 128, true)
}

/// Project a complete upstream-name set and deterministically disambiguate any
/// colliding display identities.
pub fn public_upstream_ids<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, String> {
    public_ids(names, UPSTREAM_PREFIX, 64, false)
}

/// Resolve a displayed upstream identity through the same collision-aware
/// projection used by list, catalog, and health surfaces.
pub fn resolve_public_upstream_id<'a>(
    names: impl IntoIterator<Item = &'a str>,
    requested: &str,
) -> PublicUpstreamIdResolution {
    let mut matches = public_upstream_ids(names)
        .into_iter()
        .filter(|(_, public)| public == requested);
    let Some((raw_name, public_name)) = matches.next() else {
        return PublicUpstreamIdResolution::NotFound;
    };
    if matches.next().is_some() {
        return PublicUpstreamIdResolution::Ambiguous;
    }
    PublicUpstreamIdResolution::Found {
        raw_name,
        public_name,
    }
}

/// Whether a request occupies CAS's generated public upstream namespace.
/// Unresolved values in this namespace must not be persisted as new raw keys.
pub fn is_generated_public_upstream_id(name: &str) -> bool {
    is_generated_public_id(name, UPSTREAM_PREFIX) || name.starts_with("upstream-collision-")
}

/// Project a complete tool-name set and deterministically disambiguate any
/// colliding display identities.
pub fn public_tool_ids<'a>(names: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, String> {
    public_ids(names, TOOL_PREFIX, 128, true)
}

fn public_ids<'a>(
    names: impl IntoIterator<Item = &'a str>,
    prefix: &str,
    max_len: usize,
    allow_leading_underscore: bool,
) -> BTreeMap<String, String> {
    let unique: BTreeSet<&str> = names.into_iter().collect();
    let mut projected: BTreeMap<String, String> = unique
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                public_id(name, prefix, max_len, allow_leading_underscore),
            )
        })
        .collect();

    // A raw operator name can deliberately equal another name's base
    // pseudonym. Re-project every member of a collision group with a
    // domain-separated full digest. Repeating also covers a deliberately
    // forged name equal to a prior disambiguated value.
    for round in 0_u8..8 {
        let mut by_public: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (raw, public) in &projected {
            by_public
                .entry(public.clone())
                .or_default()
                .push(raw.clone());
        }
        let collisions: Vec<Vec<String>> = by_public
            .into_values()
            .filter(|raw_names| raw_names.len() > 1)
            .collect();
        if collisions.is_empty() {
            return projected;
        }
        for raw_names in collisions {
            for raw in raw_names {
                projected.insert(
                    raw.clone(),
                    format!(
                        "{prefix}disambiguated-{}",
                        digest_hex(
                            format!("cas-public-id-v1:{round}:{prefix}:{raw}").as_bytes(),
                            32
                        )
                    ),
                );
            }
        }
    }

    // Reaching this point would require chosen SHA-256 collisions across eight
    // independent domains. Retain a deterministic, opaque, collision-free
    // projection rather than falling back to any raw identity.
    projected
        .keys()
        .enumerate()
        .map(|(index, raw)| {
            (
                raw.clone(),
                format!("{prefix}collision-{:04}", index.saturating_add(1)),
            )
        })
        .collect()
}

fn public_id(name: &str, prefix: &str, max_len: usize, allow_leading_underscore: bool) -> String {
    if is_generated_public_id(name, prefix)
        || is_safe_operator_name(name, max_len, allow_leading_underscore)
    {
        name.to_string()
    } else {
        format!("{prefix}{}", digest_hex(name.as_bytes(), 16))
    }
}

fn is_generated_public_id(name: &str, prefix: &str) -> bool {
    let Some(suffix) = name.strip_prefix(prefix) else {
        return false;
    };
    let digest = suffix.strip_prefix("disambiguated-").unwrap_or(suffix);
    matches!(digest.len(), 32 | 64)
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_operator_name(name: &str, max_len: usize, allow_leading_underscore: bool) -> bool {
    if name.is_empty() || name.len() > max_len || !name.is_ascii() {
        return false;
    }
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || (allow_leading_underscore && first == b'_'))
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return false;
    }
    if matches!(name, "." | "..") {
        return false;
    }

    let lower = name.to_ascii_lowercase();
    if lower.starts_with("ghp_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("sk-")
        || lower.starts_with("xox")
        || lower.starts_with("akia")
    {
        return false;
    }
    let normalized: String = lower
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect();
    let parts: Vec<&str> = normalized.split_whitespace().collect();
    let sensitive_part = parts.iter().any(|part| {
        matches!(
            *part,
            "token"
                | "secret"
                | "password"
                | "passwd"
                | "credential"
                | "credentials"
                | "bearer"
                | "apikey"
                | "pat"
        )
    });
    let sensitive_pair = parts.windows(2).any(|pair| {
        matches!(
            pair,
            ["api", "key"]
                | ["access", "key"]
                | ["private", "key"]
                | ["client", "secret"]
                | ["auth", "token"]
                | ["session", "token"]
        )
    });
    !sensitive_part && !sensitive_pair
}

fn digest_hex(input: &[u8], take: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(take.saturating_mul(2));
    for byte in digest.iter().take(take) {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names_are_compatible_and_unsafe_names_are_stable() {
        assert_eq!(public_upstream_id("github"), "github");
        assert_eq!(public_upstream_id("chrome-devtools"), "chrome-devtools");
        assert_eq!(public_tool_id("take_screenshot"), "take_screenshot");

        for unsafe_name in [
            "ignore\n## System",
            "/home/operator/.config/token",
            "https://user:secret@example.invalid",
            "Bearer-token",
            "production-api-key",
            "\u{001b}[31mcontrol",
        ] {
            let first = public_upstream_id(unsafe_name);
            assert!(first.starts_with(UPSTREAM_PREFIX));
            assert_eq!(first, public_upstream_id(unsafe_name));
            assert!(!first.contains(unsafe_name));
        }
    }

    #[test]
    fn collision_group_is_distinct_stable_and_idempotent() {
        let unsafe_name = "https://token@example.invalid/private";
        let base = public_upstream_id(unsafe_name);
        let first = public_upstream_ids([unsafe_name, base.as_str()]);
        let second = public_upstream_ids([base.as_str(), unsafe_name]);
        assert_eq!(first, second);
        assert_ne!(first[unsafe_name], first[&base]);
        assert_eq!(
            public_upstream_ids(first.values().map(String::as_str))
                .values()
                .cloned()
                .collect::<Vec<_>>(),
            first.values().cloned().collect::<Vec<_>>()
        );
    }

    #[test]
    fn resolver_uses_complete_collision_aware_projection_and_reserves_generated_ids() {
        let unsafe_name = "https://token@example.invalid/private";
        let forged_base = public_upstream_id(unsafe_name);
        let projected = public_upstream_ids([unsafe_name, forged_base.as_str()]);

        for raw in [unsafe_name, forged_base.as_str()] {
            assert_eq!(
                resolve_public_upstream_id(
                    [unsafe_name, forged_base.as_str()],
                    projected[raw].as_str()
                ),
                PublicUpstreamIdResolution::Found {
                    raw_name: raw.to_string(),
                    public_name: projected[raw].clone(),
                }
            );
        }
        assert_eq!(
            resolve_public_upstream_id([unsafe_name, forged_base.as_str()], &forged_base),
            PublicUpstreamIdResolution::NotFound
        );
        assert!(is_generated_public_upstream_id(&forged_base));
        assert!(is_generated_public_upstream_id(&format!(
            "upstream-disambiguated-{}",
            "a".repeat(64)
        )));
        assert!(!is_generated_public_upstream_id("safe-server"));
    }
}
