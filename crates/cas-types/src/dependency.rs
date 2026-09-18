//! Dependency type definitions
//!
//! Dependencies represent relationships between tasks in CAS.

// Dead code check enabled - all items used

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

/// Type of dependency relationship
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyType {
    /// Close-gating dependency; start warns so parallel preparation can proceed
    #[default]
    Blocks,
    /// Explicit hard start gate - task A cannot start until task B is done
    RequiresStart,
    /// Soft link - tasks are related but not blocking
    Related,
    /// Hierarchical - epic/subtask relationship
    ParentChild,
    /// Provenance - task B discovered while working on task A
    DiscoveredFrom,
    /// Rule extraction - rule extracted from task/entry work
    ExtractedFrom,
}

impl fmt::Display for DependencyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DependencyType::Blocks => write!(f, "blocks"),
            DependencyType::RequiresStart => write!(f, "requires-start"),
            DependencyType::Related => write!(f, "related"),
            DependencyType::ParentChild => write!(f, "parent-child"),
            DependencyType::DiscoveredFrom => write!(f, "discovered-from"),
            DependencyType::ExtractedFrom => write!(f, "extracted-from"),
        }
    }
}

impl FromStr for DependencyType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "blocks" => Ok(DependencyType::Blocks),
            "requires-start" | "requiresstart" => Ok(DependencyType::RequiresStart),
            "related" => Ok(DependencyType::Related),
            "parent-child" | "parentchild" => Ok(DependencyType::ParentChild),
            "discovered-from" | "discoveredfrom" => Ok(DependencyType::DiscoveredFrom),
            "extracted-from" | "extractedfrom" => Ok(DependencyType::ExtractedFrom),
            _ => Err(TypeError::Parse(format!("invalid dependency type: {s}"))),
        }
    }
}

/// A dependency relationship between two items
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// The dependent item (the one that depends on something)
    pub from_id: String,

    /// The dependency (the item being depended on)
    pub to_id: String,

    /// Type of dependency relationship
    pub dep_type: DependencyType,

    /// When the dependency was created
    pub created_at: DateTime<Utc>,

    /// Who created the dependency (for audit)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

impl Dependency {
    /// Create a new dependency
    pub fn new(from_id: String, to_id: String, dep_type: DependencyType) -> Self {
        Self {
            from_id,
            to_id,
            dep_type,
            created_at: Utc::now(),
            created_by: None,
        }
    }

    /// Check if this is a blocking dependency
    pub fn is_blocking(&self) -> bool {
        self.dep_type == DependencyType::Blocks
    }

    /// Check if this dependency is an explicit start gate.
    pub fn is_start_gate(&self) -> bool {
        self.dep_type == DependencyType::RequiresStart
    }

    /// Check if this is a hierarchical relationship
    pub fn is_hierarchical(&self) -> bool {
        self.dep_type == DependencyType::ParentChild
    }
}

impl Default for Dependency {
    fn default() -> Self {
        Self {
            from_id: String::new(),
            to_id: String::new(),
            dep_type: DependencyType::default(),
            created_at: Utc::now(),
            created_by: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::dependency::*;

    #[test]
    fn test_dependency_type_from_str() {
        assert_eq!(
            DependencyType::from_str("blocks").unwrap(),
            DependencyType::Blocks
        );
        assert_eq!(
            DependencyType::from_str("related").unwrap(),
            DependencyType::Related
        );
        assert_eq!(
            DependencyType::from_str("requires_start").unwrap(),
            DependencyType::RequiresStart
        );
        assert_eq!(
            DependencyType::from_str("parent-child").unwrap(),
            DependencyType::ParentChild
        );
        assert_eq!(
            DependencyType::from_str("discovered-from").unwrap(),
            DependencyType::DiscoveredFrom
        );
        assert_eq!(
            DependencyType::from_str("extracted-from").unwrap(),
            DependencyType::ExtractedFrom
        );
        assert!(DependencyType::from_str("invalid").is_err());
    }

    #[test]
    fn test_dependency_type_display() {
        assert_eq!(DependencyType::Blocks.to_string(), "blocks");
        assert_eq!(
            DependencyType::RequiresStart.to_string(),
            "requires-start"
        );
        assert_eq!(DependencyType::ParentChild.to_string(), "parent-child");
    }

    #[test]
    fn test_dependency_new() {
        let dep = Dependency::new(
            "cas-a1".to_string(),
            "cas-b2".to_string(),
            DependencyType::Blocks,
        );
        assert_eq!(dep.from_id, "cas-a1");
        assert_eq!(dep.to_id, "cas-b2");
        assert!(dep.is_blocking());
        assert!(!dep.is_hierarchical());
    }

    #[test]
    fn test_dependency_parent_child() {
        let dep = Dependency::new(
            "cas-child".to_string(),
            "cas-parent".to_string(),
            DependencyType::ParentChild,
        );
        assert!(!dep.is_blocking());
        assert!(dep.is_hierarchical());
    }
}
