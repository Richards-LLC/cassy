//! Helpers for retaining the ancestry of derived knowledge.

use std::collections::HashSet;

/// Return the ordered union of source IDs from several provenance groups.
///
/// Keeping insertion order makes human-facing audit output stable while the
/// set prevents repeated consolidation or promotion from growing duplicate
/// links.
pub fn merge_source_ids(groups: impl IntoIterator<Item = Vec<String>>) -> Vec<String> {
    let mut seen = HashSet::new();
    groups
        .into_iter()
        .flatten()
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::merge_source_ids;

    #[test]
    fn merge_source_ids_keeps_order_and_deduplicates() {
        assert_eq!(
            merge_source_ids(vec![
                vec!["learning-1".to_string(), "observation-1".to_string()],
                vec!["observation-1".to_string(), "learning-2".to_string()],
                vec![String::new()],
            ]),
            ["learning-1", "observation-1", "learning-2"]
        );
    }
}
