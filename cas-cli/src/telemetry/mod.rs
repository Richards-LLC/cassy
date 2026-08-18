//! Telemetry module for anonymous usage tracking
//!
//! Provides opt-in analytics via PostHog to understand Cassy usage patterns.
//! All data is anonymous (no PII, paths, or content).
//!
//! Events are sent asynchronously via a background thread to avoid blocking
//! CLI startup or command execution.

use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;

use crate::config::{Config, TelemetryConfig};
use crate::error::MemError;

/// Get PostHog API key from compile-time environment or use default
///
/// Checks in order:
/// 1. POSTHOG_API_KEY (compile-time override)
/// 2. CAS_POSTHOG_API_KEY (production key, set at compile time)
fn get_posthog_api_key() -> Option<&'static str> {
    // Allow override via environment variable at compile time
    if let Some(key) = option_env!("POSTHOG_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    // Production key (set at compile time)
    option_env!("CAS_POSTHOG_API_KEY")
}

/// Global telemetry client instance
static TELEMETRY: OnceLock<Arc<TelemetryClient>> = OnceLock::new();

/// Event to send to PostHog
struct TelemetryEvent {
    name: String,
    distinct_id: String,
    properties: Vec<(String, String)>,
}

/// Telemetry client wrapper with background sending
pub struct TelemetryClient {
    sender: Sender<TelemetryEvent>,
    anonymous_id: String,
    enabled: bool,
    cas_version: String,
    os: String,
    arch: String,
}

impl TelemetryClient {
    /// Create a new telemetry client with background worker thread
    fn new(config: &TelemetryConfig) -> Self {
        // Create channel for async event sending
        let (sender, receiver) = mpsc::channel::<TelemetryEvent>();
        let telemetry_enabled = config.enabled;

        // Spawn background thread for sending events
        thread::spawn(move || {
            // Get API key from compile-time environment - if not set, events are silently dropped
            let api_key = match get_posthog_api_key() {
                Some(key) => key.to_string(),
                None => {
                    if telemetry_enabled {
                        tracing::warn!(
                            "Telemetry enabled but no PostHog API key is configured; events will be dropped"
                        );
                    }
                    return;
                } // No API key, exit thread (events dropped)
            };

            // Build client options and create client
            let options = posthog_rs::ClientOptionsBuilder::default()
                .api_key(api_key)
                .build()
                .expect("API key is set");
            let client = posthog_rs::client(options);

            // Process events from channel until disconnected
            while let Ok(telemetry_event) = receiver.recv() {
                let mut event =
                    posthog_rs::Event::new(&telemetry_event.name, &telemetry_event.distinct_id);
                for (key, value) in telemetry_event.properties {
                    let _ = event.insert_prop(&key, &value);
                }
                // Ignore send errors - telemetry should never interrupt user flow
                let _ = client.capture(event);
            }
        });

        // Get or generate anonymous ID
        let anonymous_id = config.anonymous_id.clone().unwrap_or_else(uuid_v4);

        Self {
            sender,
            anonymous_id,
            enabled: config.enabled,
            cas_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }

    /// Get the anonymous ID
    pub fn anonymous_id(&self) -> &str {
        &self.anonymous_id
    }

    /// Check if telemetry is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Send an event to the background worker (non-blocking)
    fn send_event(&self, name: &str, properties: Vec<(&str, &str)>) {
        if !self.enabled {
            return;
        }

        let event = TelemetryEvent {
            name: name.to_string(),
            distinct_id: self.anonymous_id.clone(),
            properties: properties
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };

        // Non-blocking send - if channel is full or disconnected, just drop the event
        let _ = self.sender.send(event);
    }

    /// Track session started event
    pub fn track_session_started(&self) {
        self.send_event(
            "session_started",
            vec![
                ("cas_version", &self.cas_version),
                ("os", &self.os),
                ("arch", &self.arch),
            ],
        );
    }

    /// Track command executed event
    pub fn track_command(&self, command: &str) {
        self.send_event(
            "command_executed",
            vec![("command", command), ("cas_version", &self.cas_version)],
        );
    }

    /// Track error occurred event (sanitized, no PII)
    pub fn track_error(&self, error_type: &str, command: Option<&str>, recoverable: bool) {
        let mut props = vec![
            ("error_type", error_type),
            ("cas_version", &self.cas_version),
            ("recoverable", if recoverable { "true" } else { "false" }),
        ];
        if let Some(cmd) = command {
            props.push(("command", cmd));
        }
        self.send_event("error_occurred", props);
    }

    /// Track MCP tool called event
    pub fn track_mcp_tool(&self, tool_name: &str, action: &str, success: bool) {
        self.send_event(
            "mcp_tool_called",
            vec![
                ("tool_name", tool_name),
                ("action", action),
                ("success", if success { "true" } else { "false" }),
                ("cas_version", &self.cas_version),
            ],
        );
    }

    /// Track factory started event
    pub fn track_factory_started(&self, role: &str, worker_count: usize) {
        let count_str = worker_count.to_string();
        self.send_event(
            "factory_started",
            vec![
                ("role", role),
                ("worker_count", &count_str),
                ("cas_version", &self.cas_version),
            ],
        );
    }

    /// Track task lifecycle event (created, started, closed)
    pub fn track_task_lifecycle(&self, action: &str, task_type: Option<&str>) {
        let mut props = vec![("action", action), ("cas_version", &self.cas_version)];
        if let Some(tt) = task_type {
            props.push(("task_type", tt));
        }
        self.send_event("task_lifecycle", props);
    }

    /// Track epic completed event
    pub fn track_epic_completed(&self, task_count: usize, duration_mins: u64) {
        let count_str = task_count.to_string();
        let duration_str = duration_mins.to_string();
        self.send_event(
            "epic_completed",
            vec![
                ("task_count", &count_str),
                ("duration_mins", &duration_str),
                ("cas_version", &self.cas_version),
            ],
        );
    }

    /// Track memory added event
    pub fn track_memory_added(&self, entry_type: &str, scope: &str) {
        self.send_event(
            "memory_added",
            vec![
                ("entry_type", entry_type),
                ("scope", scope),
                ("cas_version", &self.cas_version),
            ],
        );
    }

    /// Track search performed event
    pub fn track_search_performed(&self, method: &str, result_count: usize) {
        let count_str = result_count.to_string();
        self.send_event(
            "search_performed",
            vec![
                ("method", method),
                ("result_count", &count_str),
                ("cas_version", &self.cas_version),
            ],
        );
    }

    /// Track rule promoted event
    pub fn track_rule_promoted(&self) {
        self.send_event("rule_promoted", vec![("cas_version", &self.cas_version)]);
    }

    /// Track a custom event
    pub fn track(&self, event_name: &str, properties: Vec<(&str, &str)>) {
        self.send_event(event_name, properties);
    }
}

/// Initialize the global telemetry client
///
/// Should be called early in startup. Non-blocking - spawns a background
/// thread for sending events.
/// If telemetry is disabled in config or consent hasn't been given, creates a disabled client.
pub fn init(cas_root: &Path) -> Result<(), MemError> {
    let config = Config::load(cas_root)?;
    let telemetry_config = config.telemetry.clone().unwrap_or_default();

    // Check global consent first - no telemetry until user explicitly opts in
    let global_consent = crate::config::get_telemetry_consent();
    let consent_given = global_consent.unwrap_or(false);

    // Check environment override: CAS_TELEMETRY=1 enables, takes precedence
    let env_enabled = std::env::var("CAS_TELEMETRY")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);
    let enabled = env_enabled || (consent_given && telemetry_config.enabled);

    if !enabled {
        tracing::debug!(
            consent_given,
            config_enabled = telemetry_config.enabled,
            env_enabled,
            "Telemetry disabled"
        );
    }

    let mut effective_config = telemetry_config.clone();
    effective_config.enabled = enabled;
    effective_config.consent_given = global_consent;

    // Generate and save anonymous ID if not present
    if effective_config.anonymous_id.is_none() && enabled {
        effective_config.anonymous_id = Some(uuid_v4());

        // Save the anonymous ID to config
        let mut updated_config = config.clone();
        updated_config.telemetry = Some(effective_config.clone());
        let _ = updated_config.save(cas_root);
    }

    let client = TelemetryClient::new(&effective_config);

    // Store globally (ignore if already set)
    let _ = TELEMETRY.set(Arc::new(client));

    Ok(())
}

/// Check if telemetry consent has been given
pub fn has_consent() -> bool {
    crate::config::get_telemetry_consent().unwrap_or(false)
}

/// Check if user has been asked for consent yet
pub fn consent_asked() -> bool {
    crate::config::get_telemetry_consent().is_some()
}

/// Get the global telemetry client
pub fn get() -> Option<&'static Arc<TelemetryClient>> {
    TELEMETRY.get()
}

/// Track session started (convenience function)
pub fn track_session_started() {
    if let Some(client) = get() {
        client.track_session_started();
    }
}

/// Track command executed (convenience function)
pub fn track_command(command: &str) {
    if let Some(client) = get() {
        client.track_command(command);
    }
}

/// Track error occurred (convenience function)
pub fn track_error(error_type: &str, command: Option<&str>, recoverable: bool) {
    if let Some(client) = get() {
        client.track_error(error_type, command, recoverable);
    }
}

/// Track MCP tool called (convenience function)
pub fn track_mcp_tool(tool_name: &str, action: &str, success: bool) {
    if let Some(client) = get() {
        client.track_mcp_tool(tool_name, action, success);
    }
}

/// Track factory started (convenience function)
pub fn track_factory_started(role: &str, worker_count: usize) {
    if let Some(client) = get() {
        client.track_factory_started(role, worker_count);
    }
}

/// Track task lifecycle (convenience function)
pub fn track_task_lifecycle(action: &str, task_type: Option<&str>) {
    if let Some(client) = get() {
        client.track_task_lifecycle(action, task_type);
    }
}

/// Track epic completed (convenience function)
pub fn track_epic_completed(task_count: usize, duration_mins: u64) {
    if let Some(client) = get() {
        client.track_epic_completed(task_count, duration_mins);
    }
}

/// Track memory added (convenience function)
pub fn track_memory_added(entry_type: &str, scope: &str) {
    if let Some(client) = get() {
        client.track_memory_added(entry_type, scope);
    }
}

/// Track search performed (convenience function)
pub fn track_search_performed(method: &str, result_count: usize) {
    if let Some(client) = get() {
        client.track_search_performed(method, result_count);
    }
}

/// Track rule promoted (convenience function)
pub fn track_rule_promoted() {
    if let Some(client) = get() {
        client.track_rule_promoted();
    }
}

/// Track a custom telemetry event (convenience function)
pub fn track(event_name: &str, properties: Vec<(&str, &str)>) {
    if let Some(client) = get() {
        client.track(event_name, properties);
    }
}

/// Generate a UUID v4-like random ID
fn uuid_v4() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 16] = rng.random();

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        (bytes[6] & 0x0f) | 0x40,
        bytes[7], // Version 4
        (bytes[8] & 0x3f) | 0x80,
        bytes[9], // Variant 1
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use crate::telemetry::*;

    #[test]
    fn test_uuid_v4_format() {
        let id = uuid_v4();
        // Check format: 8-4-4-4-12
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);

        // Check version nibble (should be 4)
        assert!(parts[2].starts_with('4'));
    }

    #[test]
    fn test_telemetry_client_disabled() {
        let config = TelemetryConfig {
            enabled: false,
            anonymous_id: Some("test-id".to_string()),
            consent_given: Some(false),
        };

        let client = TelemetryClient::new(&config);
        assert!(!client.is_enabled());
        assert_eq!(client.anonymous_id(), "test-id");
    }

    #[test]
    fn test_telemetry_client_enabled() {
        let config = TelemetryConfig {
            enabled: true,
            anonymous_id: Some("test-id".to_string()),
            consent_given: Some(true),
        };

        let client = TelemetryClient::new(&config);
        assert!(client.is_enabled());
    }

    #[test]
    fn test_uuid_uniqueness() {
        let id1 = uuid_v4();
        let id2 = uuid_v4();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_send_event_non_blocking() {
        let config = TelemetryConfig {
            enabled: true,
            anonymous_id: Some("test-id".to_string()),
            consent_given: Some(true),
        };

        let client = TelemetryClient::new(&config);

        // This should return immediately without blocking
        client.track_session_started();
        client.track_command("test");
        client.track_error("test_error", Some("test"), true);

        // Give the background thread a moment to process
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
