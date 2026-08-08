//! The committed, extensible query set.
//!
//! The set is a TOML file rather than a hardcoded const so it can be extended
//! without a rebuild — M1 (cas-13aa) is inventorying legacy read paths in
//! parallel, and every additional read path it finds should become additional
//! cases here rather than a code change.

use serde::{Deserialize, Serialize};

use super::{CorpusStats, DEFAULT_LIMIT, DEFAULT_RANK_TOLERANCE, ParityError};

/// A retrieval surface the harness can probe.
///
/// Each variant corresponds to a real read path in the current system; see the
/// per-variant docs for the production call site being mirrored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Tantivy BM25 unified search restricted to memories — what
    /// `mcp__cas__search action=search doc_type=entry` runs.
    Search,
    /// `mcp__cas__memory action=recent`.
    Recent,
    /// `mcp__cas__memory action=list`.
    List,
    /// The SessionStart "Pinned Memories (Always Active)" block
    /// (`Store::list_pinned`, in-context tier).
    Pinned,
    /// Feedback-ranked retrieval (`Store::list_helpful`).
    Helpful,
    /// All memories of one entry type. `query` is the type name.
    ByType,
    /// All memories in one tier. `query` is the tier name.
    ByTier,
    /// Tag-filtered retrieval. `query` is the tag.
    ByTag,
    /// The SessionStart context merge: `store.list()` on the project store
    /// then the global store, de-duplicated on the `p-`/`g-`-stripped id with
    /// project winning (`crates/cas-core/src/hooks/context/mod.rs:520`,
    /// backed by `store_list`: `archived = 0`, `LIMIT 10000`, **no tier
    /// filter** — archive-tier rows are live and still injected).
    ///
    /// This is the highest-volume reader of legacy entries, so it gets a
    /// channel of its own rather than being approximated by `list`.
    SessionMerge,
    /// `store.list()` against the **global** store alone.
    ///
    /// `session_merge` cannot stand in for this: it concatenates project rows
    /// ahead of global rows and then truncates to the case limit, so on any
    /// host whose project store is larger than that limit the global rows are
    /// cut off and the global tier contributes nothing measurable (cas-96ae).
    /// This channel reads the global store directly so global content has a
    /// recorded, diffable baseline of its own.
    ///
    /// Reports [`super::ChannelStatus::Unavailable`] when no global store is
    /// attached — a global case that silently returned zero hits would be the
    /// same blind spot in a new place.
    GlobalList,
}

impl Channel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Channel::Search => "search",
            Channel::Recent => "recent",
            Channel::List => "list",
            Channel::Pinned => "pinned",
            Channel::Helpful => "helpful",
            Channel::ByType => "by_type",
            Channel::ByTier => "by_tier",
            Channel::ByTag => "by_tag",
            Channel::SessionMerge => "session_merge",
            Channel::GlobalList => "global_list",
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCase {
    /// Stable identifier. Used as the diff key, so renaming a case reads as
    /// "old case removed, new case added" rather than a silent redefinition.
    pub id: String,
    pub channel: Channel,
    /// Channel-dependent argument: search text, type name, tier name, or tag.
    /// Ignored by the argument-less channels (recent/list/pinned/helpful).
    #[serde(default)]
    pub query: String,
    /// Result depth; falls back to the set-level default.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Per-case rank tolerance override, for queries known to be rank-unstable.
    #[serde(default)]
    pub rank_tolerance: Option<usize>,
    /// Free-text note explaining why this case is in the set.
    #[serde(default)]
    pub note: Option<String>,
}

/// The query-set file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuerySet {
    pub version: u32,
    #[serde(default = "default_limit")]
    pub default_limit: usize,
    #[serde(default = "default_tolerance")]
    pub default_rank_tolerance: usize,
    /// Content strings to drop from every result, before ranking is recorded.
    ///
    /// The live databases contain integration-test fixtures written by the
    /// test suite: five literal strings account for 994 of 1696 rows (58.6%)
    /// — see `docs/migration/cas-b129-mapping-spec.md` §3. Without this,
    /// untargeted channels (`recent`, `list`, `by_type`, `session_merge`)
    /// would baseline mostly test detritus and the harness would be asserting
    /// that the migration preserves fixtures rather than knowledge.
    ///
    /// Matching is by the same normalized fingerprint used for hits, so
    /// whitespace and case variants of a fixture string are also excluded.
    ///
    /// Excluding a row means the harness deliberately makes **no** claim about
    /// whether it survives migration. For fixtures that is the point: M3 does
    /// not drop them (rule R1 is *deliberately-leave* — not carried to pages,
    /// rows left untouched in `entries`), and their deletion is owned by a
    /// separate exact-match purge (cas-78c8 / GH #156) that may run long after
    /// cutover. Under exclusion, both their presence at replay time and their
    /// later absence after that purge are non-regressions — otherwise a
    /// routine cleanup unrelated to retrieval quality would turn the harness
    /// red.
    #[serde(default)]
    pub exclude_contents: Vec<String>,
    /// `[[query]]` tables.
    #[serde(default)]
    pub query: Vec<QueryCase>,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}
fn default_tolerance() -> usize {
    DEFAULT_RANK_TOLERANCE
}

impl QuerySet {
    pub fn parse(toml_text: &str) -> Result<Self, ParityError> {
        let set: QuerySet =
            toml::from_str(toml_text).map_err(|e| ParityError::QuerySet(e.to_string()))?;
        set.validate()?;
        Ok(set)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, ParityError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            ParityError::QuerySet(format!("cannot read query set {}: {e}", path.display()))
        })?;
        Self::parse(&raw)
    }

    fn validate(&self) -> Result<(), ParityError> {
        if self.query.is_empty() {
            return Err(ParityError::QuerySet(
                "query set contains no [[query]] cases".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for case in &self.query {
            if case.id.trim().is_empty() {
                return Err(ParityError::QuerySet("a case has an empty id".into()));
            }
            if !seen.insert(case.id.as_str()) {
                // Duplicate ids would make the report ambiguous about which
                // case regressed, and the diff would silently compare one
                // baseline case against the wrong replay case.
                return Err(ParityError::QuerySet(format!(
                    "duplicate case id '{}'",
                    case.id
                )));
            }
            let needs_arg = matches!(
                case.channel,
                Channel::Search | Channel::ByType | Channel::ByTier | Channel::ByTag
            );
            if needs_arg && case.query.trim().is_empty() {
                return Err(ParityError::QuerySet(format!(
                    "case '{}' on channel {} requires a non-empty query argument",
                    case.id, case.channel
                )));
            }
            if case.limit == Some(0) {
                return Err(ParityError::QuerySet(format!(
                    "case '{}' has limit 0, which can never detect a regression",
                    case.id
                )));
            }
        }
        Ok(())
    }

    pub fn limit_for(&self, case: &QueryCase) -> usize {
        case.limit.unwrap_or(self.default_limit)
    }

    /// Fingerprints of the excluded fixture strings, computed once per run.
    pub fn excluded_fingerprints(&self) -> std::collections::HashSet<String> {
        self.exclude_contents
            .iter()
            .map(|c| super::fingerprint(c))
            .collect()
    }

    pub fn tolerance_for(&self, case: &QueryCase, cli_override: Option<usize>) -> usize {
        case.rank_tolerance
            .or(cli_override)
            .unwrap_or(self.default_rank_tolerance)
    }

    /// Entry types and tiers present in the corpus but not probed by any case.
    ///
    /// A gap means the migration could drop everything of that type or tier
    /// and the harness would still report parity, so capture treats gaps as an
    /// error unless explicitly waived.
    pub fn coverage_gaps(&self, corpus: &CorpusStats) -> Vec<String> {
        let covered = |channel: Channel| -> std::collections::HashSet<String> {
            self.query
                .iter()
                .filter(|c| c.channel == channel)
                .map(|c| c.query.trim().to_lowercase())
                .collect()
        };
        let typed = covered(Channel::ByType);
        let tiered = covered(Channel::ByTier);

        let mut gaps = Vec::new();
        for t in &corpus.entry_types {
            if !typed.contains(&t.to_lowercase()) {
                gaps.push(format!("entry type '{t}' has no by_type case"));
            }
        }
        for t in &corpus.tiers {
            if !tiered.contains(&t.to_lowercase()) {
                gaps.push(format!("tier '{t}' has no by_tier case"));
            }
        }
        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
version = 1
[[query]]
id = "a"
channel = "by_type"
query = "learning"
"#;

    #[test]
    fn parses_minimal_set_with_defaults() {
        let set = QuerySet::parse(MINIMAL).expect("should parse");
        assert_eq!(set.version, 1);
        assert_eq!(set.default_limit, DEFAULT_LIMIT);
        assert_eq!(set.default_rank_tolerance, DEFAULT_RANK_TOLERANCE);
        assert_eq!(set.query.len(), 1);
        assert_eq!(set.query[0].channel, Channel::ByType);
        assert_eq!(set.limit_for(&set.query[0]), DEFAULT_LIMIT);
    }

    #[test]
    fn rejects_duplicate_ids() {
        let text = format!("{MINIMAL}\n[[query]]\nid = \"a\"\nchannel = \"recent\"\n");
        let err = QuerySet::parse(&text).unwrap_err().to_string();
        assert!(err.contains("duplicate case id"), "got: {err}");
    }

    #[test]
    fn rejects_empty_set() {
        let err = QuerySet::parse("version = 1\n").unwrap_err().to_string();
        assert!(err.contains("no [[query]] cases"), "got: {err}");
    }

    #[test]
    fn rejects_argument_less_search_case() {
        let err = QuerySet::parse("version = 1\n[[query]]\nid=\"a\"\nchannel=\"search\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires a non-empty query"), "got: {err}");
    }

    #[test]
    fn rejects_zero_limit() {
        let err =
            QuerySet::parse("version = 1\n[[query]]\nid=\"a\"\nchannel=\"recent\"\nlimit=0\n")
                .unwrap_err()
                .to_string();
        assert!(err.contains("limit 0"), "got: {err}");
    }

    #[test]
    fn argument_less_channels_need_no_query() {
        let set =
            QuerySet::parse("version = 1\n[[query]]\nid=\"r\"\nchannel=\"recent\"\n").unwrap();
        assert_eq!(set.query[0].channel, Channel::Recent);
    }

    #[test]
    fn tolerance_precedence_is_case_then_cli_then_default() {
        let set = QuerySet::parse(
            "version = 1\ndefault_rank_tolerance = 7\n\
             [[query]]\nid=\"a\"\nchannel=\"recent\"\nrank_tolerance=1\n\
             [[query]]\nid=\"b\"\nchannel=\"recent\"\n",
        )
        .unwrap();
        assert_eq!(set.tolerance_for(&set.query[0], Some(5)), 1, "case wins");
        assert_eq!(set.tolerance_for(&set.query[1], Some(5)), 5, "cli next");
        assert_eq!(set.tolerance_for(&set.query[1], None), 7, "then default");
    }

    #[test]
    fn coverage_gaps_flag_unprobed_types_and_tiers() {
        let set = QuerySet::parse(MINIMAL).unwrap();
        let corpus = CorpusStats {
            active_entries: 3,
            entry_types: vec!["learning".into(), "observation".into()],
            tiers: vec!["working".into()],
        };
        let gaps = set.coverage_gaps(&corpus);
        assert_eq!(gaps.len(), 2, "got: {gaps:?}");
        assert!(gaps.iter().any(|g| g.contains("observation")));
        assert!(gaps.iter().any(|g| g.contains("working")));
    }

    #[test]
    fn full_coverage_reports_no_gaps() {
        let set = QuerySet::parse(
            "version = 1\n\
             [[query]]\nid=\"t\"\nchannel=\"by_type\"\nquery=\"Learning\"\n\
             [[query]]\nid=\"w\"\nchannel=\"by_tier\"\nquery=\"working\"\n",
        )
        .unwrap();
        let corpus = CorpusStats {
            active_entries: 1,
            entry_types: vec!["learning".into()],
            tiers: vec!["working".into()],
        };
        assert!(
            set.coverage_gaps(&corpus).is_empty(),
            "matching must be case-insensitive"
        );
    }
}
