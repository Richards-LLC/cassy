//! OpenTelemetry context management for Cassy
//!
//! Provides OTEL resource attributes for correlating Claude Code telemetry
//! with Cassy sessions, agents, tasks, and projects.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const OTEL_CONTEXT_FILE: &str = "otel_context.json";

/// OTEL context for Claude Code telemetry correlation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OtelContext {
    /// Claude Code session ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Cassy agent ID (if registered)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Current task ID (if any task is in progress)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,

    /// Project canonical ID (e.g., "github.com/org/repo")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,

    /// Project path on disk
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,

    /// Cassy version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cas_version: Option<String>,

    /// Permission mode (plan, default, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,

    /// Additional custom attributes
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom_attributes: HashMap<String, String>,

    /// Timestamp when context was created
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

impl OtelContext {
    /// Create a new OTEL context with session info
    pub fn new(session_id: String) -> Self {
        Self {
            session_id: Some(session_id),
            cas_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            ..Default::default()
        }
    }

    /// Set the agent ID
    pub fn with_agent_id(mut self, agent_id: Option<String>) -> Self {
        self.agent_id = agent_id;
        self
    }

    /// Set the current task ID
    pub fn with_task_id(mut self, task_id: Option<String>) -> Self {
        self.task_id = task_id;
        self
    }

    /// Set the project ID
    pub fn with_project_id(mut self, project_id: Option<String>) -> Self {
        self.project_id = project_id;
        self
    }

    /// Set the project path
    pub fn with_project_path(mut self, project_path: Option<String>) -> Self {
        self.project_path = project_path;
        self
    }

    /// Set the permission mode
    pub fn with_permission_mode(mut self, mode: Option<String>) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Add a custom attribute
    pub fn with_attribute(mut self, key: &str, value: &str) -> Self {
        self.custom_attributes
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Write the OTEL context to the .cas directory
    pub fn write(&self, cas_root: &Path) -> std::io::Result<()> {
        let path = cas_root.join(OTEL_CONTEXT_FILE);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, json)?;
        eprintln!("cas: OTEL context written to {}", path.display());
        Ok(())
    }

    /// Read the OTEL context from the .cas directory
    pub fn read(cas_root: &Path) -> std::io::Result<Self> {
        let path = cas_root.join(OTEL_CONTEXT_FILE);
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Remove the OTEL context file
    pub fn remove(cas_root: &Path) -> std::io::Result<()> {
        let path = cas_root.join(OTEL_CONTEXT_FILE);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Convert to OTEL_RESOURCE_ATTRIBUTES format
    ///
    /// Returns a string in the format: key1=value1,key2=value2,...
    pub fn to_resource_attributes(&self) -> String {
        let mut attrs = Vec::new();

        // Standard service attributes
        attrs.push(("service.name".to_string(), "claude-code".to_string()));

        if let Some(ref version) = self.cas_version {
            attrs.push(("service.version".to_string(), version.clone()));
        }

        // Cassy-specific attributes
        if let Some(ref session_id) = self.session_id {
            attrs.push(("cas.session_id".to_string(), session_id.clone()));
        }

        if let Some(ref agent_id) = self.agent_id {
            attrs.push(("cas.agent_id".to_string(), agent_id.clone()));
        }

        if let Some(ref task_id) = self.task_id {
            attrs.push(("cas.task_id".to_string(), task_id.clone()));
        }

        if let Some(ref project_id) = self.project_id {
            attrs.push(("cas.project_id".to_string(), project_id.clone()));
        }

        if let Some(ref project_path) = self.project_path {
            attrs.push(("cas.project_path".to_string(), project_path.clone()));
        }

        if let Some(ref mode) = self.permission_mode {
            attrs.push(("cas.permission_mode".to_string(), mode.clone()));
        }

        // Custom attributes
        for (key, value) in &self.custom_attributes {
            attrs.push((format!("cas.{key}"), value.clone()));
        }

        // Format as key=value pairs, escaping special characters
        attrs
            .into_iter()
            .map(|(k, v)| format!("{}={}", k, escape_attribute_value(&v)))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Update the current task ID in the context file
    pub fn update_task_id(cas_root: &Path, task_id: Option<&str>) -> std::io::Result<()> {
        let mut ctx = Self::read(cas_root).unwrap_or_default();
        ctx.task_id = task_id.map(String::from);
        ctx.write(cas_root)
    }
}

/// Escape special characters in OTEL attribute values
fn escape_attribute_value(value: &str) -> String {
    // OTEL attribute values in the environment variable format need escaping
    // for commas and equals signs
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace('=', "\\=")
}

/// Get the current OTEL context from the .cas directory
pub fn get_otel_context(cas_root: &Path) -> Option<OtelContext> {
    OtelContext::read(cas_root).ok()
}

/// Generate OTEL_RESOURCE_ATTRIBUTES value for the current context
pub fn get_resource_attributes(cas_root: &Path) -> String {
    match OtelContext::read(cas_root) {
        Ok(ctx) => ctx.to_resource_attributes(),
        Err(_) => "service.name=claude-code".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::otel::*;
    use tempfile::TempDir;

    #[test]
    fn test_otel_context_creation() {
        let ctx = OtelContext::new("session-123".to_string())
            .with_agent_id(Some("agent-abc".to_string()))
            .with_task_id(Some("cas-1234".to_string()))
            .with_project_id(Some("github.com/org/repo".to_string()));

        assert_eq!(ctx.session_id, Some("session-123".to_string()));
        assert_eq!(ctx.agent_id, Some("agent-abc".to_string()));
        assert_eq!(ctx.task_id, Some("cas-1234".to_string()));
    }

    #[test]
    fn test_to_resource_attributes() {
        let ctx = OtelContext::new("sess-123".to_string())
            .with_agent_id(Some("agent-abc".to_string()))
            .with_task_id(Some("cas-5678".to_string()));

        let attrs = ctx.to_resource_attributes();

        assert!(attrs.contains("service.name=claude-code"));
        assert!(attrs.contains("cas.session_id=sess-123"));
        assert!(attrs.contains("cas.agent_id=agent-abc"));
        assert!(attrs.contains("cas.task_id=cas-5678"));
    }

    #[test]
    fn test_escape_special_characters() {
        let ctx = OtelContext::new("session".to_string())
            .with_project_path(Some("/path/with,comma".to_string()));

        let attrs = ctx.to_resource_attributes();

        assert!(attrs.contains("cas.project_path=/path/with\\,comma"));
    }

    #[test]
    fn test_write_and_read() {
        let temp = TempDir::new().unwrap();
        let cas_root = temp.path();

        let ctx = OtelContext::new("session-123".to_string())
            .with_agent_id(Some("agent-abc".to_string()));

        ctx.write(cas_root).unwrap();

        let read_ctx = OtelContext::read(cas_root).unwrap();
        assert_eq!(read_ctx.session_id, ctx.session_id);
        assert_eq!(read_ctx.agent_id, ctx.agent_id);
    }
}
