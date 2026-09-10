//! Size and authorization policy for high-volume factory traffic.

#[cfg(test)]
mod tests {
    use crate::config::{Config, FactoryConfig};

    use super::{
        message_body_limit, note_body_limit, source_is_exempt, validate_message_body,
        validate_note_body,
    };

    fn config_with_limits(message: usize, escalation: usize, note: usize) -> Config {
        let mut factory = FactoryConfig::default();
        factory.message_max_chars = message;
        factory.message_max_chars_escalation = escalation;
        factory.note_max_chars = note;
        Config {
            factory: Some(factory),
            ..Config::default()
        }
    }

    #[test]
    fn message_limit_switches_to_escalation_for_blockers_and_merges() {
        let config = config_with_limits(5, 11, 7);

        assert_eq!(message_body_limit(&config, false, false), 5);
        assert_eq!(message_body_limit(&config, true, false), 11);
        assert_eq!(message_body_limit(&config, false, true), 11);
    }

    #[test]
    fn message_body_accepts_exact_cap_and_rejects_cap_plus_one() {
        let config = config_with_limits(5, 11, 7);

        assert!(validate_message_body("worker", "12345", &config, false, false, "cas-449b").is_ok());
        let error = validate_message_body("worker", "123456", &config, false, false, "cas-449b")
            .expect_err("message cap must reject cap + 1");
        assert!(error.contains("limit is 5 characters"), "{error}");
        assert!(error.contains("actual length is 6"), "{error}");
        assert!(error.contains("[factory] artifacts_root/cas-449b/<name>.md"), "{error}");
        assert!(error.contains("one-paragraph summary"), "{error}");
    }

    #[test]
    fn cas_generated_message_sources_are_exempt() {
        let config = config_with_limits(5, 11, 7);
        let oversized = "123456";

        for source in ["cas", "lifecycle:task-closed", "director"] {
            assert!(source_is_exempt(source), "{source}");
            assert!(
                validate_message_body(source, oversized, &config, false, false, "cas-449b")
                    .is_ok(),
                "{source} must bypass the agent-authored cap"
            );
        }
    }

    #[test]
    fn note_body_accepts_exact_cap_and_rejects_cap_plus_one() {
        let config = config_with_limits(5, 11, 7);

        assert!(validate_note_body("progress", "1234567", &config, "cas-449b", false, false, None).is_ok());
        let error = validate_note_body(
            "blocker",
            "12345678",
            &config,
            "cas-449b",
            false,
            false,
            None,
        )
        .expect_err("note cap must reject cap + 1");
        assert!(error.contains("limit is 7 characters"), "{error}");
        assert!(error.contains("actual length is 8"), "{error}");
        assert!(error.contains("[factory] artifacts_root/cas-449b/<name>.md"), "{error}");
    }

    #[test]
    fn supervisor_override_preserves_long_review_notes_and_requires_reason() {
        let config = config_with_limits(5, 11, 7);
        let long_note = "12345678";

        assert_eq!(
            validate_note_body(
                "discovery",
                long_note,
                &config,
                "cas-449b",
                true,
                true,
                Some("merge receipt review"),
            )
            .expect("registered supervisor may preserve review evidence"),
            true
        );
        let error = validate_note_body(
            "decision",
            long_note,
            &config,
            "cas-449b",
            true,
            true,
            None,
        )
        .expect_err("override reason must be mandatory");
        assert!(error.contains("reason"), "{error}");
    }

    #[test]
    fn note_override_rejects_unregistered_callers() {
        let config = config_with_limits(5, 11, 7);
        let error = validate_note_body(
            "decision",
            "12345678",
            &config,
            "cas-449b",
            true,
            false,
            Some("review finding"),
        )
        .expect_err("only a registered supervisor may override");
        assert!(error.contains("registered supervisor"), "{error}");
    }

    #[test]
    fn configured_note_limit_is_read_from_factory() {
        let config = config_with_limits(5, 11, 7);

        assert_eq!(note_body_limit(&config), 7);
    }
}
