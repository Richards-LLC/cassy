use crate::entry::*;

#[test]
fn test_entry_type_from_str() {
    assert_eq!(
        EntryType::from_str("learning").unwrap(),
        EntryType::Learning
    );
    assert_eq!(
        EntryType::from_str("PREFERENCE").unwrap(),
        EntryType::Preference
    );
    assert_eq!(EntryType::from_str("Context").unwrap(), EntryType::Context);
    assert_eq!(
        EntryType::from_str("observation").unwrap(),
        EntryType::Observation
    );
    assert!(EntryType::from_str("invalid").is_err());
}

#[test]
fn test_new_observation() {
    let entry = Entry::new_observation(
        "2024-01-15-001".to_string(),
        "Observed file write".to_string(),
        "session-123".to_string(),
        "Write".to_string(),
    );
    assert_eq!(entry.entry_type, EntryType::Observation);
    assert_eq!(entry.session_id, Some("session-123".to_string()));
    assert_eq!(entry.source_tool, Some("Write".to_string()));
    assert!(entry.pending_extraction);
}

#[test]
fn test_feedback_score() {
    let mut entry = Entry::new("test".to_string(), "content".to_string());
    entry.helpful_count = 5;
    entry.harmful_count = 2;
    assert_eq!(entry.feedback_score(), 3);
}

#[test]
fn test_preview() {
    let entry = Entry::new(
        "test".to_string(),
        "This is a long content string".to_string(),
    );
    assert_eq!(entry.preview(10), "This is...");
    assert_eq!(entry.preview(100), "This is a long content string");
}

#[test]
fn test_temporal_validity() {
    use chrono::Duration;

    let mut entry = Entry::new("test".to_string(), "content".to_string());

    // No validity bounds - always valid
    assert!(entry.is_temporally_valid());
    assert!(!entry.is_expired());

    // Set validity to past period (expired)
    let past = Utc::now() - Duration::days(10);
    let yesterday = Utc::now() - Duration::days(1);
    entry.set_validity(Some(past), Some(yesterday));
    assert!(!entry.is_temporally_valid());
    assert!(entry.is_expired());

    // Set validity to future period (not yet valid)
    let tomorrow = Utc::now() + Duration::days(1);
    let future = Utc::now() + Duration::days(10);
    entry.set_validity(Some(tomorrow), Some(future));
    assert!(!entry.is_temporally_valid());
    assert!(!entry.is_expired());

    // Set validity to current period (valid)
    let past = Utc::now() - Duration::days(1);
    let future = Utc::now() + Duration::days(1);
    entry.set_validity(Some(past), Some(future));
    assert!(entry.is_temporally_valid());
    assert!(!entry.is_expired());

    // Only valid_from (in past) - valid
    entry.set_validity(Some(past), None);
    assert!(entry.is_temporally_valid());
    assert!(!entry.is_expired());

    // Only valid_until (in future) - valid
    entry.set_validity(None, Some(future));
    assert!(entry.is_temporally_valid());
    assert!(!entry.is_expired());

    // Only valid_until (in past) - expired
    entry.set_validity(None, Some(yesterday));
    assert!(!entry.is_temporally_valid());
    assert!(entry.is_expired());
}

#[test]
fn superseded_memories_are_expired_even_without_a_deadline() {
    for (content, tags, superseded) in [
        ("SUPERSEDED 2026-10-03: the operator raised the cap to 10 via Commander #14942 (\"Cap it at 10\") while working on prowl (macOS). Use a maximum of 10 concurrent workers.", vec![], false),
        ("Old gate", vec!["superseded"], true),
        ("Old gate", vec![" SUPERSEDED-BY:mem-new "], true),
        ("SUPERSEDED by the current policy", vec![], false),
        ("A discussion of superseded rules", vec![], false),
        ("Supersededness is a topic", vec![], false),
    ] {
        let mut entry = Entry::new("policy".into(), content.into());
        entry.tags = tags.into_iter().map(str::to_owned).collect();
        assert_eq!(entry.is_expired(), superseded, "{entry:?}");
        assert_eq!(entry.is_temporally_valid(), !superseded, "{entry:?}");
        entry.expire_if_superseded(Utc::now());
        assert_eq!(entry.valid_until.is_some(), superseded, "prose must not persist retirement: {entry:?}");
    }
}
