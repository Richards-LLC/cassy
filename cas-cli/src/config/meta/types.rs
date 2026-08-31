/// Type of configuration value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigType {
    /// Boolean (true/false)
    Bool,
    /// Integer number
    Int,
    /// Floating-point number
    Float,
    /// String value
    String,
    /// Comma-separated list of strings
    StringList,
}

impl ConfigType {
    /// Get a human-readable type name
    pub fn name(&self) -> &'static str {
        match self {
            ConfigType::Bool => "boolean",
            ConfigType::Int => "integer",
            ConfigType::Float => "number",
            ConfigType::String => "string",
            ConfigType::StringList => "string list",
        }
    }

    /// Get example values for this type
    pub fn examples(&self) -> Vec<&'static str> {
        match self {
            ConfigType::Bool => vec!["true", "false"],
            ConfigType::Int => vec!["10", "100", "1000"],
            ConfigType::Float => vec!["0.5", "0.9", "1.0"],
            ConfigType::String => vec!["value", "path/to/file"],
            ConfigType::StringList => vec!["item1,item2,item3"],
        }
    }
}

/// Validation constraints for a config value
#[derive(Debug, Clone)]
pub enum Constraint {
    /// No constraints
    None,
    /// Minimum integer value
    Min(i64),
    /// Maximum integer value
    Max(i64),
    /// Range (inclusive)
    Range(i64, i64),
    /// Must be one of these values
    OneOf(Vec<String>),
    /// Must not be empty
    NotEmpty,
    /// Must be a valid path
    ValidPath,
}

/// Metadata for a single configuration option
#[derive(Debug, Clone)]
pub struct ConfigMeta {
    /// The full key path (e.g., "hooks.token_budget")
    pub key: &'static str,
    /// Section this belongs to
    pub section: &'static str,
    /// Human-readable display name
    pub name: &'static str,
    /// Detailed description
    pub description: &'static str,
    /// Value type
    pub value_type: ConfigType,
    /// Default value as string
    pub default: &'static str,
    /// Validation constraint
    pub constraint: Constraint,
    /// Whether this is an advanced option (hidden by default in simple lists)
    pub advanced: bool,
    /// Feature flag required (if any)
    pub requires_feature: Option<&'static str>,
    /// Keywords for semantic search (synonyms, related terms)
    pub keywords: &'static [&'static str],
    /// Common use cases when you'd change this setting
    pub use_cases: &'static [&'static str],
}

impl ConfigMeta {
    /// Validate a value against this config's constraints
    pub fn validate(&self, value: &str) -> Result<(), String> {
        // First check type
        match self.value_type {
            ConfigType::Bool => {
                if !["true", "false", "1", "0", "yes", "no"]
                    .contains(&value.to_lowercase().as_str())
                {
                    return Err(format!("Expected boolean (true/false), got: {value}"));
                }
            }
            ConfigType::Int => {
                let parsed: Result<i64, _> = value.parse();
                if parsed.is_err() {
                    return Err(format!("Expected integer, got: {value}"));
                }
                let num = parsed.unwrap();

                // Check constraints
                match &self.constraint {
                    Constraint::Min(min) => {
                        if num < *min {
                            return Err(format!("Value must be at least {min}, got: {num}"));
                        }
                    }
                    Constraint::Max(max) => {
                        if num > *max {
                            return Err(format!("Value must be at most {max}, got: {num}"));
                        }
                    }
                    Constraint::Range(min, max) => {
                        if num < *min || num > *max {
                            return Err(format!(
                                "Value must be between {min} and {max}, got: {num}"
                            ));
                        }
                    }
                    _ => {}
                }
            }
            ConfigType::Float => {
                let parsed: Result<f64, _> = value.parse();
                if parsed.is_err() {
                    return Err(format!("Expected number, got: {value}"));
                }
            }
            ConfigType::String | ConfigType::StringList => match &self.constraint {
                Constraint::NotEmpty => {
                    if value.trim().is_empty() {
                        return Err("Value cannot be empty".to_string());
                    }
                }
                Constraint::OneOf(options) => {
                    if !options.iter().any(|o| o.eq_ignore_ascii_case(value)) {
                        return Err(format!("Value must be one of: {}", options.join(", ")));
                    }
                }
                _ => {}
            },
        }

        Ok(())
    }

    /// Format value for display
    pub fn format_value(&self, value: &str) -> String {
        match self.value_type {
            ConfigType::Bool => {
                if value == "true" || value == "1" || value.to_lowercase() == "yes" {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            ConfigType::StringList => {
                // Format as comma-separated, handling empty
                if value.is_empty() {
                    "(empty)".to_string()
                } else {
                    value.to_string()
                }
            }
            _ => value.to_string(),
        }
    }

    /// Check if value differs from default
    pub fn is_modified(&self, value: &str) -> bool {
        value != self.default
    }
}
