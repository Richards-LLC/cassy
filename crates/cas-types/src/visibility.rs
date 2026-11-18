//! Visibility type for controlling access to shared data
//!
//! Visibility determines who can see entries, tasks, rules, and other data
//! when shared via team workspaces.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

/// Visibility level for shared data
///
/// Controls who can access entries, tasks, rules, and other data
/// when shared via team workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Only the creator can see (default)
    #[default]
    Private,

    /// All team members can see
    Team,

    /// All organization members can see
    Organization,

    /// Anyone with the link can see (future use)
    Public,
}

impl Visibility {
    /// Check if this is private visibility
    pub fn is_private(&self) -> bool {
        *self == Visibility::Private
    }

    /// Check if this is team visibility
    pub fn is_team(&self) -> bool {
        *self == Visibility::Team
    }

    /// Check if this is organization visibility
    pub fn is_organization(&self) -> bool {
        *self == Visibility::Organization
    }

    /// Check if this is public visibility
    pub fn is_public(&self) -> bool {
        *self == Visibility::Public
    }

    /// Check if this visibility allows access for team members
    pub fn allows_team_access(&self) -> bool {
        matches!(
            self,
            Visibility::Team | Visibility::Organization | Visibility::Public
        )
    }

    /// Check if this visibility allows access for organization members
    pub fn allows_org_access(&self) -> bool {
        matches!(self, Visibility::Organization | Visibility::Public)
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Visibility::Private => write!(f, "private"),
            Visibility::Team => write!(f, "team"),
            Visibility::Organization => write!(f, "organization"),
            Visibility::Public => write!(f, "public"),
        }
    }
}

impl FromStr for Visibility {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "private" | "priv" => Ok(Visibility::Private),
            "team" => Ok(Visibility::Team),
            "organization" | "org" => Ok(Visibility::Organization),
            "public" | "pub" => Ok(Visibility::Public),
            _ => Err(TypeError::Parse(format!("Invalid visibility: {}", s))),
        }
    }
}
