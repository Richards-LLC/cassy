//! Nearest-match suggestions for unknown skill names (GH #810, cas-d019).
//!
//! An operator says "ExaSearchSkill"; the skill is `exa-search`. The old
//! answer was a bare "not found", which cost a directory listing per session.
//! Matching is case- and separator-insensitive with a trailing `skill`
//! stripped, then a small Levenshtein bound over the listed names.

/// Normalise a skill name for comparison: lowercase, drop `-`/`_`/spaces,
/// and drop a trailing `skill` (people say "the exa search skill").
pub fn normalize_skill_name(name: &str) -> String {
    let compact: String = name
        .chars()
        .filter(|ch| !matches!(ch, '-' | '_' | ' ' | '.'))
        .flat_map(char::to_lowercase)
        .collect();
    compact
        .strip_suffix("skill")
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
        .unwrap_or(compact)
}

/// Classic Levenshtein distance over chars.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Candidates worth naming for `query`, best first, at most three.
///
/// Exact normalised matches win outright; otherwise a candidate qualifies when
/// its normalised form is within `max(2, len/4)` edits of the query, or one
/// contains the other (so "search" finds `exa-search`). Duplicates collapse.
pub fn suggest_skill_names<'a, I>(query: &str, candidates: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let wanted = normalize_skill_name(query);
    if wanted.is_empty() {
        return Vec::new();
    }
    let bound = (wanted.chars().count() / 4).max(2);
    let mut scored: Vec<(usize, String)> = Vec::new();
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() || scored.iter().any(|(_, seen)| seen == candidate) {
            continue;
        }
        let normal = normalize_skill_name(candidate);
        let score = if normal == wanted {
            0
        } else if normal.contains(&wanted) || wanted.contains(&normal) {
            1
        } else {
            let distance = levenshtein(&wanted, &normal);
            if distance > bound {
                continue;
            }
            distance + 1
        };
        scored.push((score, candidate.to_owned()));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

/// The full "not found" sentence, with the suggestion clause only when there
/// is one: `Skill not found: ExaSearchSkill. Did you mean: exa-search?`
pub fn unknown_skill_message<'a, I>(query: &str, candidates: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let suggestions = suggest_skill_names(query, candidates);
    if suggestions.is_empty() {
        format!("Skill not found: {query}")
    } else {
        format!(
            "Skill not found: {query}. Did you mean: {}?",
            suggestions.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: [&str; 6] = [
        "exa-search",
        "cas-search",
        "cas-worker",
        "cas-supervisor",
        "release-notes",
        "mecha-cassy",
    ];

    #[test]
    fn camel_case_with_skill_suffix_finds_the_kebab_name() {
        assert_eq!(
            unknown_skill_message("ExaSearchSkill", CATALOG),
            "Skill not found: ExaSearchSkill. Did you mean: exa-search?"
        );
        assert_eq!(suggest_skill_names("exa_search", CATALOG), ["exa-search"]);
        assert_eq!(suggest_skill_names("Exa Search", CATALOG), ["exa-search"]);
    }

    #[test]
    fn small_typos_and_substrings_still_suggest() {
        assert_eq!(suggest_skill_names("exa-serch", CATALOG), ["exa-search"]);
        assert_eq!(
            suggest_skill_names("search", CATALOG),
            ["cas-search", "exa-search"]
        );
        assert_eq!(
            suggest_skill_names("releasenotes", CATALOG),
            ["release-notes"]
        );
    }

    #[test]
    fn far_names_get_no_suggestion_and_message_stays_bare() {
        assert!(suggest_skill_names("kubernetes", CATALOG).is_empty());
        assert_eq!(
            unknown_skill_message("kubernetes", CATALOG),
            "Skill not found: kubernetes"
        );
        assert!(suggest_skill_names("", CATALOG).is_empty());
    }

    #[test]
    fn levenshtein_is_the_textbook_distance() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }
}
