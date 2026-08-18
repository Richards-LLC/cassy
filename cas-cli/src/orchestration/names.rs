//! Friendly name generator for Cassy agents
//!
//! Generates random adjective-noun combinations like "jolly-panda" or "swift-falcon"
//! for use as human-friendly agent identifiers in multi-agent sessions.

use rand::Rng;
use rand::seq::IndexedRandom;
use std::collections::HashSet;

// ── Minions theme names ──────────────────────────────────────────────────────

const MINION_WORKERS: &[&str] = &[
    "kevin", "stuart", "bob", "dave", "jerry", "tim", "mark", "phil", "carl",
    "norbert", "jorge", "otto", "steve", "herb", "pete", "donnie", "mel",
    "abel", "tony", "walter",
];

const MINION_SUPERVISORS: &[&str] = &["gru", "dru", "nefario"];

/// Generate a single minion worker name (e.g., "kevin", "stuart")
pub fn generate_minion() -> String {
    let mut rng = rand::rng();
    let name = MINION_WORKERS.choose(&mut rng).unwrap_or(&"bob");
    (*name).to_string()
}

/// Generate a minion supervisor name
pub fn generate_minion_supervisor() -> String {
    let mut rng = rand::rng();
    let name = MINION_SUPERVISORS.choose(&mut rng).unwrap_or(&"gru");
    (*name).to_string()
}

/// Generate N unique minion worker names.
///
/// If more names are requested than available, appends a numeric suffix.
pub fn generate_minion_unique(count: usize) -> Vec<String> {
    let mut names = Vec::with_capacity(count);
    let mut rng = rand::rng();

    // Shuffle the pool and take as many as we can
    let mut pool: Vec<&str> = MINION_WORKERS.to_vec();
    // Fisher-Yates shuffle
    for i in (1..pool.len()).rev() {
        let j = rng.random_range(0..=i);
        pool.swap(i, j);
    }

    for (i, name) in pool.iter().enumerate() {
        if i >= count {
            break;
        }
        names.push((*name).to_string());
    }

    // If we need more than the pool, add suffixed duplicates
    let mut suffix = 2;
    while names.len() < count {
        for name in &pool {
            if names.len() >= count {
                break;
            }
            names.push(format!("{name}-{suffix}"));
        }
        suffix += 1;
    }

    names
}

// ── Default theme names ──────────────────────────────────────────────────────

const ADJECTIVES: &[&str] = &[
    "agile", "bold", "brave", "bright", "calm", "clever", "cosmic", "crisp", "daring", "eager",
    "fair", "fast", "fierce", "gentle", "golden", "happy", "jolly", "keen", "kind", "lively",
    "loyal", "mighty", "nimble", "noble", "patient", "proud", "quick", "quiet", "rapid", "ready",
    "sharp", "silent", "smooth", "solid", "steady", "strong", "sturdy", "subtle", "swift",
    "tender", "true", "vivid", "warm", "watchful", "wild", "wise", "witty", "young", "zealous",
    "zen",
];

const NOUNS: &[&str] = &[
    "badger", "bear", "cardinal", "cheetah", "cobra", "condor", "crane", "crow", "dolphin",
    "dragon", "eagle", "falcon", "finch", "fox", "gazelle", "gopher", "hawk", "heron", "hound",
    "jaguar", "jay", "kestrel", "koala", "lark", "leopard", "lion", "lynx", "marten", "merlin",
    "newt", "octopus", "otter", "owl", "panda", "panther", "parrot", "pelican", "phoenix", "puma",
    "raven", "robin", "salmon", "shark", "sparrow", "spider", "stork", "swan", "tiger", "viper",
    "wolf",
];

/// Generate a single random friendly name
///
/// # Example
/// ```rust
/// use cas::orchestration::names;
///
/// let name = names::generate();
/// // Returns something like "jolly-panda-42" or "swift-falcon-7"
/// ```
pub fn generate() -> String {
    let mut rng = rand::rng();
    let adj = ADJECTIVES.choose(&mut rng).unwrap_or(&"swift");
    let noun = NOUNS.choose(&mut rng).unwrap_or(&"agent");
    let num: u8 = rng.random_range(1..100);
    format!("{adj}-{noun}-{num}")
}

/// Generate N unique friendly names
///
/// If more names are requested than possible combinations (50 * 51 * 99 = 252,450),
/// this will return as many unique names as possible.
///
/// # Example
/// ```rust
/// use cas::orchestration::names;
///
/// let names = names::generate_unique(3);
/// // Returns something like ["jolly-panda-42", "swift-falcon-7", "brave-owl-88"]
/// ```
pub fn generate_unique(count: usize) -> Vec<String> {
    let max_combinations = ADJECTIVES.len() * NOUNS.len() * 99;
    let count = count.min(max_combinations);

    let mut names = HashSet::with_capacity(count);
    let mut attempts = 0;
    let max_attempts = count * 10; // Prevent infinite loop

    while names.len() < count && attempts < max_attempts {
        names.insert(generate());
        attempts += 1;
    }

    names.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use crate::orchestration::names::*;

    #[test]
    fn test_generate_format() {
        let name = generate();
        assert!(name.contains('-'), "Name should contain hyphen: {name}");
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(
            parts.len(),
            3,
            "Name should have exactly three parts: {name}"
        );
    }

    #[test]
    fn test_generate_uses_valid_words() {
        let name = generate();
        let parts: Vec<&str> = name.split('-').collect();
        assert!(
            ADJECTIVES.contains(&parts[0]),
            "First part should be a valid adjective: {}",
            parts[0]
        );
        assert!(
            NOUNS.contains(&parts[1]),
            "Second part should be a valid noun: {}",
            parts[1]
        );
        let num: u8 = parts[2].parse().expect("Third part should be a number");
        assert!((1..100).contains(&num), "Number should be 1-99: {num}");
    }

    #[test]
    fn test_generate_unique_returns_correct_count() {
        let names = generate_unique(5);
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn test_generate_unique_all_different() {
        let names = generate_unique(10);
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "All names should be unique");
    }

    #[test]
    fn test_generate_unique_handles_large_count() {
        // Should not panic even with large count
        let names = generate_unique(100);
        assert_eq!(names.len(), 100);
    }

    #[test]
    fn test_generate_minion_returns_valid_name() {
        let name = generate_minion();
        assert!(
            MINION_WORKERS.contains(&name.as_str()),
            "Minion name should be valid: {name}"
        );
    }

    #[test]
    fn test_generate_minion_supervisor_returns_valid_name() {
        let name = generate_minion_supervisor();
        assert!(
            MINION_SUPERVISORS.contains(&name.as_str()),
            "Supervisor name should be valid: {name}"
        );
    }

    #[test]
    fn test_generate_minion_unique_returns_correct_count() {
        let names = generate_minion_unique(5);
        assert_eq!(names.len(), 5);
    }

    #[test]
    fn test_generate_minion_unique_all_different() {
        let names = generate_minion_unique(10);
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "All minion names should be unique");
    }

    #[test]
    fn test_generate_minion_unique_exceeds_pool() {
        // More than 20 minion names, should use suffixes
        let names = generate_minion_unique(25);
        assert_eq!(names.len(), 25);
        let unique: HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), 25, "All names should still be unique");
    }
}
