//! Content terms for task-create context and epic scope comparison.

const QUERY_TERM_CAP: usize = 16;

pub(super) fn content_terms(text: &str) -> Vec<String> {
    terms_from_text(text)
}

fn terms_from_text(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut previous = String::new();
    for raw in
        text.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.')))
    {
        let term = raw.trim_matches(['-', '_', '/', '.']).to_ascii_lowercase();
        if term.is_empty() {
            continue;
        }
        // cas-b62d: the number in a harness envelope's "message 33981" or
        // "notification 33981" is a delivery id, not content. A plain number
        // elsewhere (an issue, PR or error code) stays a term.
        let envelope_id = matches!(previous.as_str(), "message" | "notification")
            && term.chars().all(|ch| ch.is_ascii_digit());
        previous.clone_from(&term);
        if envelope_id {
            continue;
        }
        if is_content_bearing_term(&term) && !terms.contains(&term) {
            terms.push(term);
            if terms.len() == QUERY_TERM_CAP {
                break;
            }
        }
    }
    terms
}

fn is_high_document_frequency_term(term: &str) -> bool {
    const STOP: &[&str] = &[
        "role",
        "worker",
        "supervisor",
        "project",
        "task",
        "task_title",
        "labels",
        "files",
        "symbols",
        "decisions",
        "already_seen",
        "request",
        "this",
        "that",
        "them",
        "what",
        "which",
        "when",
        "where",
        "who",
        "why",
        "how",
        "the",
        "and",
        "are",
        "was",
        "were",
        "been",
        "being",
        "have",
        "has",
        "had",
        "does",
        "did",
        "can",
        "could",
        "would",
        "should",
        "for",
        "with",
        "from",
        "into",
        "about",
        "after",
        "before",
        "between",
        "because",
        "over",
        "under",
        "again",
        "then",
        "use",
        "open",
        "need",
        "want",
        "get",
        "got",
        "make",
        "made",
        "also",
        "just",
        "more",
        "most",
        "some",
        "such",
        "only",
        "very",
        "will",
        "queue",
        "new",
        "old",
        "first",
        "decision",
        "context",
        "check",
        "out",
        "put",
        "merge",
        "branch",
        "every",
        "please",
        "implement",
        // cas-3c98 (GH #991, a regression of #188/#213): function words and
        // tool words that alone manufactured matches such as "you,cli",
        // "its,next" and "cas,not".
        "you",
        "your",
        "yours",
        "our",
        "its",
        "not",
        "any",
        "all",
        "each",
        "other",
        "here",
        "there",
        "they",
        "their",
        "these",
        "those",
        "than",
        "but",
        "now",
        "yes",
        "via",
        "per",
        "may",
        "might",
        "must",
        "let",
        "one",
        "see",
        "add",
        "next",
        "cli",
        "cas",
        // Harness envelope words: every CAS wake, teammate message and task
        // notification carries them, so they say nothing about the request
        // ("task-notification, status, summary"; "wake, message, inbox, see").
        "message",
        "messages",
        "summary",
        "status",
        "notification",
        "wake",
        "inbox",
        "body",
        "repeated",
        "teammate",
        "teammate-message",
        "teammate_id",
        "task-notification",
        "task-id",
        "tool-use-id",
        "output-file",
        "system-reminder",
        "redelivery",
        "replay",
        "supervisor-authored",
        "agent-authored",
    ];
    STOP.contains(&term)
}

/// cas-3c98 (GH #213 regression): a short token of mixed hex letters and
/// digits ("e7e", "4f2", "a94") is an id shard, not a word; it matches
/// arbitrary hashes in stored text. Plain numbers (issue, PR and error codes)
/// and seven or more hex characters (a commit-sha prefix) stay searchable.
fn is_short_hex_fragment(term: &str) -> bool {
    term.len() < 7
        && term.chars().all(|ch| ch.is_ascii_hexdigit())
        && term.chars().any(|ch| ch.is_ascii_digit())
        && term.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_content_bearing_term(term: &str) -> bool {
    term.len() >= 3 && !is_high_document_frequency_term(term) && !is_short_hex_fragment(term)
}
