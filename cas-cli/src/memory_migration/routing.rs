//! Row-level routing — the ordered decision procedure of spec §4 (R1–R9).
//!
//! First match wins, and the procedure is **total**: every legacy row reaches
//! exactly one disposition. A row that matches nothing is a hard error
//! (§11 assert 7), never an "unaccounted" bucket entry — that is what makes the
//! zero-loss audit fall out of the routing rather than being asserted on top
//! of it.

use anyhow::{Result, bail};

use super::source::LegacyRow;

/// What happens to a row. Spec §"Disposition vocabulary".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Disposition {
    /// Becomes an unlocked `knowledge_pages` row + markdown body.
    MigrateToPage,
    /// Becomes a **locked** page whose body is byte-identical to the content.
    CarryVerbatim,
    /// Remains in `entries`, untouched.
    StayEntry,
    /// Folded into another row's destination rather than getting its own page.
    MergedInto,
    /// Intentionally not carried; §4/§5 state what is lost and why.
    DeliberatelyLeave,
}

impl Disposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MigrateToPage => "migrate-to-page",
            Self::CarryVerbatim => "carry-verbatim",
            Self::StayEntry => "stay-entry",
            Self::MergedInto => "merged-into",
            Self::DeliberatelyLeave => "deliberately-leave",
        }
    }

    /// Does this disposition produce a `knowledge_pages` row?
    pub fn writes_a_page(self) -> bool {
        matches!(self, Self::MigrateToPage | Self::CarryVerbatim)
    }
}

/// The outcome of the decision procedure for one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub rule: &'static str,
    pub disposition: Disposition,
    /// Set only by R6; the token that triggered the quarantine.
    pub quarantine_token: Option<String>,
}

/// R1 — the five integration-test fixture strings that account for 994 of 1696
/// rows (§3). Matched by **exact equality**; a `LIKE` here would eat real
/// memories that merely quote a fixture.
pub const FIXTURE_CONTENTS: [&str; 5] = [
    "Test memory from MCP protocol test",
    "Rust programming language with ownership and borrowing",
    "Context test memory entry",
    "Consolidated memory test entry",
    "Test entry for notification test",
];

/// R6 tokens matched as case-sensitive **substrings** — proper nouns and a form
/// name, implausible inside cas-src prose.
///
/// The first five are the original E1 list (accounting/tax vocabulary). The
/// remainder are the **property/loan/settlement widening** added after the M5
/// cutover rehearsal, where 29 of the 181 migrated pages turned out to be
/// another project's real-estate records — and two of them (a HUD-1 and a
/// settlement statement) won the SessionStart knowledge-index budget and were
/// injected into the top of every cas-src session. Those rows carry none of the
/// original five tokens, so R6 let them through.
///
/// Every added token is a **proper noun** — an entity, lender or property name.
/// Generic English words were deliberately rejected: measured against the real
/// 1700-row corpus, `Property` matches `Object.getOwnPropertyNames(...)`,
/// `Lease` matches the `LeaseNotFound` error type, and `Richards` matches the
/// `Richards-LLC` GitHub org and Vercel team that appear throughout genuine
/// cas-src infrastructure memories. None of those three are here. Each of
/// `Escrow`, `Mortgage`, `Loan`, `Settlement Statement`, `Warranty Deed`,
/// `1098`, `K-1`, `CSL`, `Ingram` and `Rearden` was also measured and adds
/// **zero** rows beyond the proper nouns below, so none earns a place.
const SUBSTRING_TOKENS: [&str; 14] = [
    // E1 original list — accounting/tax vocabulary.
    "QBO",
    "TNTAP",
    "FONCE",
    "FAE 183",
    "Journal Entr",
    // Property/loan/settlement widening — entity, lender and property names.
    "Roark",
    "Realty",
    "JRPW",
    "Renovo",
    "Leake",
    "Moultrie",
    "Radnor",
    "Old Hickory",
    "HUD-1",
];

/// R6 tokens matched only as **standalone words**.
///
/// Supervisor amendment to escalation E1: `1040` and `1065` as substrings would
/// quarantine any cas-src memory that cites `file.rs:1040`. "Standalone" here is
/// deliberately stricter than a regex `\b`, which still matches inside
/// `file.rs:1040` because `:` is a non-word character.
const WORD_TOKENS: [&str; 2] = ["1040", "1065"];

/// Characters that may precede a standalone digit token. Note the exclusions:
/// `:` `/` `.` `-` `_` `#` all appear in file paths, line references and
/// version strings, which is exactly the false positive E1 was amended to
/// prevent.
const LEADING_OK: [char; 5] = ['(', '[', '"', '\'', '\u{201c}'];

/// Characters that may follow a standalone digit token — sentence punctuation
/// only. `-` is excluded so `1040-1065` (a range, e.g. an index or line span)
/// does not match.
const TRAILING_OK: [char; 10] = ['.', ',', ';', ':', ')', ']', '"', '\'', '!', '?'];

fn is_standalone(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = haystack[from..].find(token) {
        let start = from + offset;
        let end = start + token.len();

        let leading_ok = match haystack[..start].chars().next_back() {
            None => true,
            Some(ch) => ch.is_whitespace() || LEADING_OK.contains(&ch),
        };
        let trailing_ok = match haystack[end..].chars().next() {
            None => true,
            Some(ch) => ch.is_whitespace() || TRAILING_OK.contains(&ch),
        };
        if leading_ok && trailing_ok {
            return true;
        }
        // Advance past this occurrence, staying on a char boundary.
        from = start + 1;
        while from < bytes.len() && !haystack.is_char_boundary(from) {
            from += 1;
        }
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// The R6 contamination predicate over a row's title and content (§4.1).
///
/// Returns the first matching token in a fixed order so the quarantine ledger
/// is deterministic across runs.
pub fn contamination_token(title: &str, content: &str) -> Option<String> {
    for token in SUBSTRING_TOKENS {
        if title.contains(token) || content.contains(token) {
            return Some(token.to_string());
        }
    }
    for token in WORD_TOKENS {
        if is_standalone(title, token) || is_standalone(content, token) {
            return Some(token.to_string());
        }
    }
    None
}

fn route_of(rule: &'static str, disposition: Disposition) -> Route {
    Route {
        rule,
        disposition,
        quarantine_token: None,
    }
}

/// Apply R1–R9 top to bottom, first match wins.
pub fn route(row: &LegacyRow) -> Result<Route> {
    // R1 — integration-test fixtures. Exact equality, never LIKE.
    if FIXTURE_CONTENTS.contains(&row.content.as_str()) {
        return Ok(route_of("R1", Disposition::DeliberatelyLeave));
    }
    // R2 — pins. Highest-privilege read path; content survives untouched.
    if row.memory_tier == "in_context" {
        return Ok(route_of("R2", Disposition::CarryVerbatim));
    }
    // R3 — preferences are persona/user-sovereign content.
    if row.entry_type == "preference" {
        return Ok(route_of("R3", Disposition::CarryVerbatim));
    }
    // R4 — epistemic state has live mutators with no destination equivalent.
    if row.belief_type == "opinion" || row.belief_type == "hypothesis" {
        return Ok(route_of("R4", Disposition::StayEntry));
    }
    // R5 — observations are session ephemera, already second-class on read.
    if row.entry_type == "observation" {
        return Ok(route_of("R5", Disposition::StayEntry));
    }
    // R6 — cross-project contamination: quarantine in place, never delete.
    if let Some(token) = contamination_token(row.title.as_deref().unwrap_or(""), &row.content) {
        return Ok(Route {
            rule: "R6",
            disposition: Disposition::DeliberatelyLeave,
            quarantine_token: Some(token),
        });
    }
    // R7 — durable project facts.
    if row.entry_type == "context" {
        return Ok(route_of("R7", Disposition::MigrateToPage));
    }
    if row.entry_type == "learning" {
        // R8 — positively-reinforced learning is the corpus's only
        // human-confirmed durability signal. Importance is explicitly rejected
        // as a routing signal (§3: it is partly a contamination signal).
        if row.helpful_count > row.harmful_count {
            return Ok(route_of("R8", Disposition::MigrateToPage));
        }
        // R9 — remaining session learnings.
        return Ok(route_of("R9", Disposition::StayEntry));
    }

    // §11 assert 7. Do not invent a disposition for an unknown type.
    bail!(
        "row {} has entry type '{}', which matches no rule in the §4 decision \
         procedure — the migration aborts rather than guess a disposition",
        row.id,
        row.entry_type
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_rejects_path_and_version_neighbours() {
        for negative in [
            "cas-cli/src/lib.rs:1040",
            "line1040",
            "1040x",
            "v1.1040",
            "1040-1065",
            "id_1040",
            "#1040",
            "a/1040",
        ] {
            assert!(
                !is_standalone(negative, "1040"),
                "{negative} must not match"
            );
        }
    }

    #[test]
    fn standalone_accepts_prose_occurrences() {
        for positive in [
            "1040",
            "Richards 1040 filed",
            "the 1040.",
            "(1040)",
            "form 1040?",
        ] {
            assert!(is_standalone(positive, "1040"), "{positive} must match");
        }
    }

    #[test]
    fn standalone_scans_past_a_rejected_first_occurrence() {
        assert!(is_standalone(
            "lib.rs:1040 and also form 1040 filed",
            "1040"
        ));
    }

    #[test]
    fn substring_tokens_are_case_sensitive() {
        assert_eq!(contamination_token("", "QBO ledger"), Some("QBO".into()));
        assert_eq!(contamination_token("", "qbo ledger"), None);
    }

    /// The rows that made the widening necessary: real page titles from the M5
    /// rehearsal that the original five-token predicate let through.
    #[test]
    fn the_property_widening_catches_the_rows_that_reached_sessionstart() {
        for (title, expected) in [
            (
                "105 Leake Ave #44 — Original Purchase Settlement (08/31/2022)",
                "Leake",
            ),
            (
                "202 Moultrie Park — Original Purchase HUD-1 (08/18/2022) — CSL Loan $1.33M",
                "Moultrie",
            ),
            (
                "Bargain Sale PSA — Friends of Radnor Lake, $1,825,000 for both OH properties",
                "Radnor",
            ),
            ("Roark Property Basis and Assessor Splits", "Roark"),
            (
                "JRPW GP Note Details — Confirmed from Forbearance Agreement + Payoff Docs",
                "JRPW",
            ),
            ("Renovo Loans Confirmed Interest-Only — Ben", "Renovo"),
            ("CSL 806 Old Hickory 1098 Data (2023-2025)", "Old Hickory"),
        ] {
            assert_eq!(
                contamination_token(title, ""),
                Some(expected.into()),
                "{title} must quarantine on {expected}"
            );
        }
    }

    /// Generic English words were rejected because the REAL corpus contains
    /// these exact cas-src strings. A regression here would quarantine genuine
    /// engineering memories — the mirror-image failure of the one being fixed.
    #[test]
    fn genuine_cas_src_prose_is_never_quarantined() {
        for legitimate in [
            // `Property` would match this — an actual cas-src jest/SWC memory.
            "Check via Object.getOwnPropertyNames(ItemsService.prototype).length",
            // `Lease` would match this typed error from task cas-6cb0.
            "cas-6cb0 (P3, typed LeaseNotFound) — follow-up",
            // `Richards` would match the GitHub org and the Vercel team; both
            // appear across live cas-src infrastructure memories.
            "**Repo:** Richards-LLC/petra-stella-cloud (private)",
            "Richards-LLC Vercel team ID is `team_oYRRKRaKgk6MHCVv`",
            // `Loan`/`Deed`/`Settlement` as generic words would match these.
            "download the artifact and check indeed the settlement of the queue",
        ] {
            assert_eq!(
                contamination_token("", legitimate),
                None,
                "{legitimate} is genuine cas-src content and must NOT quarantine"
            );
        }
    }

    #[test]
    fn token_precedence_is_deterministic() {
        // Both a substring token and a word token present: the substring token
        // wins, always, so the quarantine ledger is reproducible.
        assert_eq!(
            contamination_token("", "QBO for the 1040 return"),
            Some("QBO".into())
        );
    }

    #[test]
    fn empty_inputs_never_quarantine() {
        assert_eq!(contamination_token("", ""), None);
    }

    #[test]
    fn dispositions_round_trip_their_labels() {
        assert_eq!(Disposition::MigrateToPage.as_str(), "migrate-to-page");
        assert!(Disposition::CarryVerbatim.writes_a_page());
        assert!(!Disposition::StayEntry.writes_a_page());
        assert!(!Disposition::DeliberatelyLeave.writes_a_page());
    }
}
