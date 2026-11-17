//! Find or file (cas-7823): the key that matches a new Cassy report or
//! finding to an open issue or task before anything new is filed.

const FINGERPRINT_MARKER: &str = "**Fingerprint**: `";

/// Lowercased words with ids, numbers and hashes collapsed to `#` and gate
/// tags removed, so one refusal reads the same in every report.
fn normalize_refusal(text: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || c == '`') {
        let word = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if word.is_empty() || crate::gates::find(word).is_some() {
            continue;
        }
        let word = if word.chars().any(|c| c.is_ascii_digit()) || raw.contains('…') {
            "#".to_string()
        } else {
            word.to_lowercase()
        };
        if !(word == "#" && words.last().is_some_and(|last| last == "#")) {
            words.push(word);
        }
    }
    words.join(" ")
}

/// The find-or-file key: the refusal's registry gate codes plus its
/// normalized headline. Too little text is no key, so nothing matches it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RefusalFingerprint {
    codes: Vec<&'static str>,
    headline: String,
}

fn is_distinctive(headline: &str) -> bool {
    headline.split(' ').filter(|word| *word != "#").count() >= 4
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    format!(" {haystack} ").contains(&format!(" {needle} "))
}

impl RefusalFingerprint {
    /// A report leads `actual` with the verbatim refusal: key on its first line.
    pub(crate) fn of(refusal: &str) -> Option<Self> {
        Self::keyed(refusal, refusal.lines())
    }

    /// A finding's repro ends with the wrong output: key on its last line.
    pub(crate) fn of_last(repro: &str) -> Option<Self> {
        Self::keyed(repro, repro.lines().rev())
    }

    fn keyed<'a>(text: &str, mut lines: impl Iterator<Item = &'a str>) -> Option<Self> {
        let headline = lines.find_map(|line| Some(normalize_refusal(line)).filter(|line| !line.is_empty()))?;
        is_distinctive(&headline).then(|| Self {
            codes: crate::gates::codes_in(text),
            headline,
        })
    }

    /// Same codes, and one headline contains the other: the earlier report's
    /// text, or the fingerprint it recorded, names this refusal.
    pub(crate) fn matches(&self, candidate: &str) -> bool {
        if !self.codes.iter().all(|code| candidate.contains(code)) {
            return false;
        }
        contains_phrase(&normalize_refusal(candidate), &self.headline)
            || candidate.match_indices(FINGERPRINT_MARKER).any(|(start, _)| {
                let rest = &candidate[start + FINGERPRINT_MARKER.len()..];
                rest.split('`').next().is_some_and(|stored| {
                    is_distinctive(stored) && contains_phrase(&self.headline, stored)
                })
            })
    }

    pub(crate) fn marker(&self) -> String {
        format!("- {FINGERPRINT_MARKER}{}`", self.headline)
    }
}

/// The first open task whose title, description or notes name this refusal.
pub(crate) fn open_task_match(
    store: &dyn cas_store::TaskStore,
    fingerprint: &RefusalFingerprint,
) -> anyhow::Result<Option<crate::types::Task>> {
    Ok(store.list(None)?.into_iter().find(|task| {
        !task.is_terminal()
            && fingerprint.matches(&format!("{}\n{}\n{}", task.title, task.description, task.notes))
    }))
}
