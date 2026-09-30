//! The schema and evaluator for the single reasoned builtin prose registry.
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

fn sensitive() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    version: u32,
    documents: BTreeMap<String, Document>,
    alternatives: Vec<Alternative>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    source: String,
    catalogs: Vec<String>,
    contains: Vec<Rule>,
    absent: Vec<Rule>,
    any_of: Vec<AnyOf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    text: String,
    reason: String,
    #[serde(default = "sensitive")]
    case_sensitive: bool,
    #[serde(default)]
    unicode_case: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnyOf {
    texts: Vec<String>,
    reason: String,
    #[serde(default = "sensitive")]
    case_sensitive: bool,
    #[serde(default)]
    unicode_case: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Alternative {
    choices: Vec<Choice>,
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    document: String,
    text: String,
    absent: bool,
    case_sensitive: bool,
}

fn nonempty(value: &str) -> bool {
    !value.trim().is_empty()
}

fn relative(value: &str) -> bool {
    nonempty(value)
        && !value.starts_with('/')
        && !value.split('/').any(|part| part == "..")
        && !value.contains('\\')
}

impl Policy {
    pub fn parse(text: &str) -> Result<Self, String> {
        let policy: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if policy.version != 2 || policy.documents.is_empty() {
            return Err("expected version 2 and nonempty documents".into());
        }
        let mut referenced = BTreeSet::new();
        for alternative in &policy.alternatives {
            if !nonempty(&alternative.reason) || alternative.choices.is_empty() {
                return Err("every alternative needs choices and a reason".into());
            }
            for choice in &alternative.choices {
                if !nonempty(&choice.text) || !policy.documents.contains_key(&choice.document) {
                    return Err("alternative needs a known document and nonempty phrase".into());
                }
                referenced.insert(choice.document.clone());
            }
        }
        for (path, document) in &policy.documents {
            if !relative(path) || !relative(&document.source) {
                return Err(format!("{path}: invalid relative document/source path"));
            }
            let catalogs: BTreeSet<_> = document.catalogs.iter().collect();
            if catalogs.is_empty()
                || catalogs.len() != document.catalogs.len()
                || catalogs
                    .iter()
                    .any(|c| !matches!(c.as_str(), "claude" | "codex" | "grok" | "opencode"))
            {
                return Err(format!("{path}: invalid catalogs"));
            }
            for rule in document.contains.iter().chain(&document.absent) {
                if !nonempty(&rule.text)
                    || !nonempty(&rule.reason)
                    || (rule.unicode_case && rule.case_sensitive)
                {
                    return Err(format!("{path}: every phrase needs text and a reason"));
                }
            }
            for rule in &document.any_of {
                if (rule.unicode_case && rule.case_sensitive)
                    || !nonempty(&rule.reason)
                    || rule.texts.is_empty()
                    || !rule.texts.iter().all(|t| nonempty(t))
                {
                    return Err(format!("{path}: alternatives need phrases and a reason"));
                }
            }
            if document.contains.is_empty()
                && document.absent.is_empty()
                && document.any_of.is_empty()
                && !referenced.contains(path)
            {
                return Err(format!("{path}: empty contract"));
            }
        }
        Ok(policy)
    }

    /// Resolve shipped content, never the checkout. Missing catalog entries
    /// fail in the lookup; optional documents explicitly declare their catalogs.
    pub fn failures(&self, lookup: impl Fn(&str, &str) -> String) -> Vec<String> {
        let mut failures = Vec::new();
        for catalog in ["claude", "codex", "grok", "opencode"] {
            let texts: BTreeMap<_, _> = self
                .documents
                .iter()
                .filter(|(_, doc)| doc.catalogs.iter().any(|c| c == catalog))
                .map(|(path, _)| (path.clone(), lookup(catalog, path)))
                .collect();
            for (path, text) in &texts {
                let document = &self.documents[path];
                for (kind, rules, expected) in [
                    ("contains", &document.contains, true),
                    ("absent", &document.absent, false),
                ] {
                    for rule in rules {
                        if present(text, &rule.text, rule.case_sensitive, rule.unicode_case)
                            != expected
                        {
                            failures.push(format!(
                                "{catalog} {path}: {kind} {:?}: {}",
                                rule.text, rule.reason
                            ));
                        }
                    }
                }
                for rule in &document.any_of {
                    if !rule
                        .texts
                        .iter()
                        .any(|p| present(text, p, rule.case_sensitive, rule.unicode_case))
                    {
                        failures.push(format!(
                            "{catalog} {path}: any_of {:?}: {}",
                            rule.texts, rule.reason
                        ));
                    }
                }
            }
            for alternative in &self.alternatives {
                let choices: Vec<_> = alternative
                    .choices
                    .iter()
                    .filter(|choice| texts.contains_key(&choice.document))
                    .collect();
                if !choices.is_empty()
                    && !choices.iter().any(|choice| {
                        present(
                            &texts[&choice.document],
                            &choice.text,
                            choice.case_sensitive,
                            false,
                        ) != choice.absent
                    })
                {
                    failures.push(format!("{catalog}: alternative: {}", alternative.reason));
                }
            }
        }
        failures
    }
}

fn present(text: &str, phrase: &str, case_sensitive: bool, unicode_case: bool) -> bool {
    if case_sensitive {
        text.contains(phrase)
    } else if unicode_case {
        text.to_lowercase().contains(&phrase.to_lowercase())
    } else {
        text.to_ascii_lowercase()
            .contains(&phrase.to_ascii_lowercase())
    }
}
