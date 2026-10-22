//! Independent runs (cas-d863). A product can dedupe a resubmission and answer
//! with the same in-flight result, so N recorded runs may be fewer real ones.
//! A journey's `independence_key` names the result identity (for example
//! `post_audit_v2_runs.id`); each run row records `<key>=<id>`, and only
//! distinct ids count toward the runs a gate requires.
use std::collections::BTreeMap;

/// The value of the first `<key>=<id>` token in a row's cells.
pub fn identity<'a>(key: &str, cells: &[&'a str]) -> Option<&'a str> {
    let token = format!("{key}=");
    cells.iter().find_map(|cell| {
        cell.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '`'))
            .find_map(|word| word.strip_prefix(token.as_str()))
            .filter(|id| !id.is_empty())
    })
}

/// Run rows for one journey, counted by identity.
#[derive(Debug, Default, PartialEq)]
pub struct Runs {
    pub recorded: usize,
    /// Each identity and how many rows recorded it.
    pub identities: BTreeMap<String, usize>,
    /// Rows that recorded no identity.
    pub unidentified: usize,
}

impl Runs {
    pub fn count<'a>(rows: impl IntoIterator<Item = Option<&'a str>>) -> Self {
        let mut runs = Self::default();
        for id in rows {
            runs.recorded += 1;
            match id {
                Some(id) => *runs.identities.entry(id.into()).or_default() += 1,
                None => runs.unidentified += 1,
            }
        }
        runs
    }

    pub fn independent(&self) -> usize {
        self.identities.len()
    }

    /// Why these runs do not satisfy `required` independent runs: a duplicate
    /// or unidentified row never counts, and each duplicate is named.
    pub fn refusal(&self, journey: &str, key: &str, required: usize) -> Option<String> {
        let duplicates: Vec<String> = self
            .identities
            .iter()
            .filter(|(_, rows)| **rows > 1)
            .map(|(id, rows)| format!("duplicate {key}={id} ({rows} rows)"))
            .collect();
        if duplicates.is_empty() && self.unidentified == 0 && self.independent() >= required {
            return None;
        }
        let mut causes = duplicates;
        if self.unidentified > 0 {
            causes.push(format!("{} without {key}=<id>", self.unidentified));
        }
        if self.independent() < required {
            causes.push(format!("{required} required"));
        }
        Some(format!(
            "{} {journey} runs recorded, {} independent by {key}: {}",
            self.recorded,
            self.independent(),
            causes.join("; ")
        ))
    }
}
