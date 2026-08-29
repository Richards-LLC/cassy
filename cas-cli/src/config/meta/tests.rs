use crate::config::meta::*;

#[test]
fn test_registry_has_all_sections() {
    let reg = ConfigRegistry::new();
    let sections = reg.sections();

    assert!(sections.contains(&"sync"));
    assert!(sections.contains(&"cloud"));
    assert!(sections.contains(&"hooks"));
    assert!(sections.contains(&"hooks.stop"));
    assert!(sections.contains(&"hooks.plan_mode"));
    assert!(sections.contains(&"tasks"));
    assert!(sections.contains(&"issues"));
    assert!(sections.contains(&"dev"));
    assert!(sections.contains(&"notifications"));
    assert!(sections.contains(&"notifications.tasks"));
    assert!(sections.contains(&"notifications.entries"));
    assert!(sections.contains(&"notifications.rules"));
    assert!(sections.contains(&"notifications.skills"));
    // New sections
    assert!(sections.contains(&"coordination"));
    assert!(sections.contains(&"lease"));
    assert!(sections.contains(&"telemetry"));
}

#[test]
fn test_registry_count() {
    let reg = ConfigRegistry::new();
    // Should have 50 options across all sections (33 + 17 notification options)
    assert!(
        reg.count() >= 50,
        "Expected 50+ config options, got {}",
        reg.count()
    );
}

#[test]
fn test_validate_bool() {
    let reg = ConfigRegistry::new();

    assert!(reg.validate("sync.enabled", "true").is_ok());
    assert!(reg.validate("sync.enabled", "false").is_ok());
    assert!(reg.validate("sync.enabled", "yes").is_ok());
    assert!(reg.validate("sync.enabled", "invalid").is_err());
    assert!(reg.validate("sync.promotion_threshold", "2").is_ok());
    assert!(reg.validate("sync.promotion_threshold", "1").is_err());
    assert!(reg.validate("sync.demotion_threshold", "2").is_ok());
    assert!(reg.validate("sync.demotion_threshold", "1").is_err());
}

#[test]
fn test_validate_int_range() {
    let reg = ConfigRegistry::new();

    assert!(reg.validate("cloud.interval_secs", "300").is_ok());
    assert!(reg.validate("cloud.interval_secs", "30").is_ok());
    assert!(reg.validate("cloud.interval_secs", "3600").is_ok());
    assert!(reg.validate("cloud.interval_secs", "9").is_err()); // Below min
    assert!(reg.validate("cloud.interval_secs", "10000").is_err()); // Above max
    assert!(reg.validate("cloud.queue_pending_warning", "200").is_ok());
    assert!(reg.validate("cloud.queue_pending_warning", "0").is_err());
    assert!(reg.validate("cloud.queue_oldest_warning_secs", "21600").is_ok());
    assert!(reg.validate("cloud.queue_oldest_warning_secs", "59").is_err());
}

#[test]
fn test_search() {
    let reg = ConfigRegistry::new();

    let results = reg.search("token");
    assert!(results.len() >= 2); // hooks.token_budget and hooks.plan_mode.token_budget
}

#[test]
fn test_section_keys() {
    let reg = ConfigRegistry::new();

    let sync_keys = reg.section_keys("sync");
    assert!(sync_keys.contains(&"sync.enabled"));
    assert!(sync_keys.contains(&"sync.target"));
    assert!(sync_keys.contains(&"sync.min_helpful"));
    assert!(sync_keys.contains(&"sync.promotion_threshold"));
    assert!(sync_keys.contains(&"sync.demotion_threshold"));
    assert!(sync_keys.contains(&"sync.promotion_evidence"));
}

#[test]
fn test_is_modified() {
    let reg = ConfigRegistry::new();
    let meta = reg.get("sync.enabled").unwrap();

    assert!(!meta.is_modified("true"));
    assert!(meta.is_modified("false"));
}
