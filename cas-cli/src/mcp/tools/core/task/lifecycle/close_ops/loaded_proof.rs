//! One field assessment drives loaded-proof acceptance and repair diagnostics.
use super::{has_at_least_three_runs, has_parallelism_16, has_passing_result, has_target_scope};

const NAMES: [&str; 4] = ["target", "parallelism", "loops", "result"];
const REQUIREMENTS: [&str; 4] = [
    "whole/entire/full target or explicit non-Rust target (target=<scope>)",
    "parallelism at 16 jobs (-j16, --jobs 16, parallelism=16 or concurrency=16)",
    "at least 3 loops/runs",
    "passing result (PASS, SUCCESS, exit 0, or status 0)",
];
const EXAMPLE: &str = "LOADED_PROOF: target=explicit non-Rust (vitest src/example.test.ts + jest tests/example.test.js) parallelism=16 loops=3 result=PASS";

pub(super) fn has_structured_target_scope(lower: &str) -> bool {
    lower.split([';', ',', '|']).any(|field| {
        let mut words = field.split_whitespace();
        while let Some(word) = words.next() {
            let Some(scope) = word.strip_prefix("target=") else {
                continue;
            };
            let scope = scope.trim_matches(['\'', '"', '`']);
            if matches!(
                scope,
                "whole" | "entire" | "full" | "vitest" | "jest" | "pytest"
            ) {
                return true;
            }
            if scope == "go" && words.next() == Some("test") {
                return true;
            }
            if scope == "explicit" {
                let next = words.next();
                if next == Some("non-rust") || (next == Some("non") && words.next() == Some("rust"))
                {
                    return true;
                }
            }
        }
        false
    })
}

pub(super) struct Fields([bool; 4]);

impl Fields {
    pub(super) fn complete(&self) -> bool {
        self.0.iter().all(|present| *present)
    }

    fn describe(&self) -> String {
        let names = |present| {
            NAMES
                .iter()
                .zip(self.0)
                .filter_map(|(name, parsed)| (parsed == present).then_some(*name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let requirements = REQUIREMENTS
            .iter()
            .zip(self.0)
            .filter_map(|(requirement, parsed)| (!parsed).then_some(*requirement))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "parsed=[{}] missing=[{}] ({requirements})",
            names(true),
            names(false)
        )
    }
}

pub(super) fn assess_notes(notes: &str) -> impl Iterator<Item = Fields> + '_ {
    notes.lines().filter_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("loaded_proof").then(|| {
            Fields([
                has_target_scope(&lower),
                has_parallelism_16(&lower),
                has_at_least_three_runs(&lower),
                has_passing_result(&lower),
            ])
        })
    })
}

pub(super) fn missing_evidence(notes: &str) -> String {
    let assessments = assess_notes(notes)
        .enumerate()
        .map(|(index, fields)| format!("note {}: {}", index + 1, fields.describe()))
        .collect::<Vec<_>>();
    let detail = if assessments.is_empty() {
        format!(
            "typed note_type=loaded_proof; {}",
            Fields([false; 4]).describe()
        )
    } else {
        // Report each note independently: combining partial notes cannot prove
        // that any single run had all four required fields.
        format!(
            "all required evidence on the same typed note; {}",
            assessments.join("; ")
        )
    };
    format!("{detail}. Accepted example: {EXAMPLE}")
}
