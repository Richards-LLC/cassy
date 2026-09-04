//! GH #669 — the client's canonical project identity must be byte-identical to
//! the cloud server's.
//!
//! The vectors live in `tests/fixtures/canonical_project_identity_vectors.json`
//! and are lifted verbatim from the server's own
//! `tests/lib/project-identity.test.ts` (`Richards-LLC/petra-stella-cloud`
//! @ `ee422ceceb688edb2c6736037988924638a4dfa1`), plus the alias spellings the
//! production alias-merge migration actually moved.
//!
//! Why a shared file rather than an inline table: identity is a *contract*
//! between two repositories. When the server adds a vector, this file is the
//! single artifact that has to be copied, and a drift shows up as a failing
//! client test instead of as silently forked cloud buckets.


use cas::cloud::{canonical_project_id, project_ids_match_with_aliases};

fn vectors() -> serde_json::Value {
    // Embedded at compile time: CI suite shards run on a different runner than
    // the build, so a CARGO_MANIFEST_DIR path does not exist there.
    serde_json::from_str(include_str!("fixtures/canonical_project_identity_vectors.json"))
        .unwrap_or_else(|e| panic!("canonical_project_identity_vectors.json is not valid JSON: {e}"))
}

#[test]
fn client_canonicalization_matches_the_server_vectors_byte_for_byte() {
    let doc = vectors();
    let cases = doc["syntax"]
        .as_array()
        .expect("fixture must carry a `syntax` array");
    assert!(
        cases.len() >= 19,
        "the shared vector shrank to {} cases — a deleted vector is a silently forked bucket",
        cases.len()
    );

    let mut failures = Vec::new();
    for case in cases {
        let expected = case["expected"].as_str().map(str::to_string);
        // A JSON `null` input models the server's `typeof raw !== "string"`
        // branch, which the Rust signature makes unrepresentable; the empty
        // string exercises the same `null` return.
        let Some(input) = case["input"].as_str() else {
            assert!(
                expected.is_none(),
                "a non-string input must canonicalize to null"
            );
            continue;
        };
        let actual = canonical_project_id(input);
        if actual != expected {
            failures.push(format!(
                "  {input:?}\n    expected {expected:?}\n    actual   {actual:?}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "client/server canonicalization drift ({} case(s)):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The registry-level fold is *not* a syntax rule: `ozer-health` and `ozer` do
/// not converge under any normalizer. Only the per-project `aliases` record
/// makes them one project, which is exactly why the client has to consume it.
#[test]
fn alias_families_do_not_fold_by_syntax_but_do_fold_through_the_registry_record() {
    let doc = vectors();
    let families = doc["alias_families"]["families"]
        .as_array()
        .expect("fixture must carry alias_families.families");
    assert!(!families.is_empty());

    for family in families {
        let canonical = family["canonical_id"].as_str().expect("canonical_id");
        let aliases: Vec<String> = family["aliases"]
            .as_array()
            .expect("aliases")
            .iter()
            .map(|a| a.as_str().expect("alias string").to_string())
            .collect();

        // The class the client caches is the canonical id plus every alias.
        let mut class = aliases.clone();
        class.push(canonical.to_string());

        for alias in &aliases {
            assert_ne!(
                canonical_project_id(alias).as_deref(),
                Some(canonical),
                "{alias} must not collapse onto {canonical} by syntax — if it \
                 does, the alias record is not what is doing the work and the \
                 normalizer is guessing"
            );
            // v3.9.0 already folds a remote-shaped alias onto a bare pin when
            // the repository name is the pin (`…/gabber-studio` → `gabber-studio`).
            // The families that rule cannot reach are the ones where the
            // repository was *renamed* — `ozer-health` under `ozer` — and those
            // must be measurably foreign until the record is consumed. That is
            // the concrete bug GH #669 reports.
            let last_segment = alias.rsplit('/').next().unwrap_or(alias);
            if last_segment != canonical {
                assert!(
                    !project_ids_match_with_aliases(alias, canonical, &[]),
                    "without the registered record, `{alias}` must still read as \
                     foreign to `{canonical}`"
                );
            }
            assert!(
                project_ids_match_with_aliases(alias, canonical, &class),
                "the registered alias record must attribute `{alias}` to `{canonical}`"
            );
            assert!(
                project_ids_match_with_aliases(canonical, alias, &class),
                "alias attribution must hold in both directions"
            );
        }
    }
}

/// `penguinz` / `pippenz` are legacy catch-all buckets the server migration
/// deliberately left untouched. Nothing the client does may fold them into a
/// repository project, with or without an alias record.
#[test]
fn unmapped_catch_all_buckets_are_never_folded_into_a_repository_project() {
    let doc = vectors();
    let unmapped: Vec<String> = doc["alias_families"]["unmapped"]["identities"]
        .as_array()
        .expect("unmapped.identities")
        .iter()
        .map(|v| v.as_str().expect("identity").to_string())
        .collect();
    assert_eq!(unmapped, vec!["penguinz", "pippenz"]);

    let registered = vec![
        "gabber-studio".to_string(),
        "github.com/richards-llc/gabber-studio".to_string(),
    ];
    for identity in &unmapped {
        assert!(
            !project_ids_match_with_aliases(identity, "gabber-studio", &registered),
            "{identity} is an unmapped catch-all and must stay foreign to gabber-studio"
        );
    }
}
