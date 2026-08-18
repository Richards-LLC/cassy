use std::collections::HashMap;

use crate::config::meta::seed::populate_registry;
use crate::config::meta::types::{ConfigMeta, Constraint};

/// Registry of all configuration options
pub struct ConfigRegistry {
    /// All config metadata indexed by key
    pub(crate) configs: HashMap<&'static str, ConfigMeta>,
    /// Config keys organized by section
    pub(crate) sections: HashMap<&'static str, Vec<&'static str>>,
    /// Section descriptions
    pub(crate) section_descriptions: HashMap<&'static str, &'static str>,
}

impl ConfigRegistry {
    pub(crate) fn empty() -> Self {
        Self {
            configs: HashMap::new(),
            sections: HashMap::new(),
            section_descriptions: HashMap::new(),
        }
    }

    /// Create the complete config registry with all options
    pub fn new() -> Self {
        let mut registry = Self::empty();
        populate_registry(&mut registry);
        registry
    }

    /// Register a config option
    pub(crate) fn register(&mut self, meta: ConfigMeta) {
        self.sections
            .entry(meta.section)
            .or_default()
            .push(meta.key);
        self.configs.insert(meta.key, meta);
    }

    /// Get metadata for a config key
    pub fn get(&self, key: &str) -> Option<&ConfigMeta> {
        self.configs.get(key)
    }

    /// Get all config keys
    pub fn all_keys(&self) -> Vec<&'static str> {
        self.configs.keys().copied().collect()
    }

    /// Get keys for a section
    pub fn section_keys(&self, section: &str) -> Vec<&'static str> {
        self.sections.get(section).cloned().unwrap_or_default()
    }

    /// Get all section names
    pub fn sections(&self) -> Vec<&'static str> {
        let mut sections: Vec<_> = self.sections.keys().copied().collect();
        // Sort with parent sections before subsections
        sections.sort_by(|a, b| {
            let a_depth = a.matches('.').count();
            let b_depth = b.matches('.').count();
            if a_depth != b_depth {
                a_depth.cmp(&b_depth)
            } else {
                a.cmp(b)
            }
        });
        sections
    }

    /// Get section description
    pub fn section_description(&self, section: &str) -> Option<&'static str> {
        self.section_descriptions.get(section).copied()
    }

    /// Get all configs in a section
    pub fn configs_in_section(&self, section: &str) -> Vec<&ConfigMeta> {
        self.section_keys(section)
            .iter()
            .filter_map(|key| self.configs.get(key))
            .collect()
    }

    /// Search configs by keyword (searches key, name, description, and keywords)
    pub fn search(&self, query: &str) -> Vec<&ConfigMeta> {
        let query_lower = query.to_lowercase();
        self.configs
            .values()
            .filter(|meta| {
                meta.key.to_lowercase().contains(&query_lower)
                    || meta.name.to_lowercase().contains(&query_lower)
                    || meta.description.to_lowercase().contains(&query_lower)
                    || meta
                        .keywords
                        .iter()
                        .any(|k| k.to_lowercase().contains(&query_lower))
                    || meta
                        .use_cases
                        .iter()
                        .any(|u| u.to_lowercase().contains(&query_lower))
            })
            .collect()
    }

    /// Get count of all config options
    pub fn count(&self) -> usize {
        self.configs.len()
    }

    /// Get only non-advanced configs
    pub fn basic_configs(&self) -> Vec<&ConfigMeta> {
        self.configs.values().filter(|m| !m.advanced).collect()
    }

    /// Get only advanced configs
    pub fn advanced_configs(&self) -> Vec<&ConfigMeta> {
        self.configs.values().filter(|m| m.advanced).collect()
    }

    /// Validate a value for a key
    pub fn validate(&self, key: &str, value: &str) -> Result<(), String> {
        match self.get(key) {
            Some(meta) => meta.validate(value),
            None => Err(format!("Unknown config key: {key}")),
        }
    }

    /// Generate markdown documentation for all config options
    pub fn generate_markdown(&self) -> String {
        let mut md = String::new();

        // Header
        md.push_str("# Cassy Configuration Reference\n\n");
        md.push_str("Configuration file: `.cas/config.toml`\n\n");

        // Table of Contents
        md.push_str("## Table of Contents\n\n");
        for section in self.sections() {
            let anchor = section.replace('.', "");
            let desc = self.section_description(section).unwrap_or("");
            md.push_str(&format!("- [{section}](#{anchor})"));
            if !desc.is_empty() {
                md.push_str(&format!(" - {desc}"));
            }
            md.push('\n');
        }
        md.push_str("\n---\n\n");

        // Generate each section
        for section in self.sections() {
            let anchor = section.replace('.', "");
            md.push_str(&format!("## {section} {{#{anchor}}}\n\n"));

            if let Some(desc) = self.section_description(section) {
                md.push_str(&format!("{desc}\n\n"));
            }

            // Get configs for this section, sorted by key
            let mut configs: Vec<_> = self.configs_in_section(section);
            configs.sort_by_key(|c| c.key);

            for meta in configs {
                self.write_config_markdown(&mut md, meta);
            }
        }

        md
    }

    /// Write markdown for a single config option
    fn write_config_markdown(&self, md: &mut String, meta: &ConfigMeta) {
        // Frontmatter-style header (for agent parsing)
        md.push_str("---\n");
        md.push_str(&format!("key: {}\n", meta.key));
        md.push_str(&format!("section: {}\n", meta.section));
        md.push_str(&format!("type: {}\n", meta.value_type.name()));
        md.push_str(&format!("default: {}\n", meta.default));
        if !meta.keywords.is_empty() {
            md.push_str(&format!("keywords: [{}]\n", meta.keywords.join(", ")));
        }
        if !meta.use_cases.is_empty() {
            md.push_str(&format!("use_cases: [{}]\n", meta.use_cases.join(", ")));
        }
        md.push_str("---\n\n");

        // Config key as heading
        md.push_str(&format!("### {}\n\n", meta.key));

        // Name and description
        md.push_str(&format!("**{}**\n\n", meta.name));
        md.push_str(&format!("{}\n\n", meta.description));

        // Properties table
        md.push_str("| Property | Value |\n");
        md.push_str("|----------|-------|\n");
        md.push_str(&format!("| Type | `{}` |\n", meta.value_type.name()));
        md.push_str(&format!("| Default | `{}` |\n", meta.default));

        // Constraint
        match &meta.constraint {
            Constraint::None => {}
            Constraint::Min(min) => {
                md.push_str(&format!("| Min | `{min}` |\n"));
            }
            Constraint::Max(max) => {
                md.push_str(&format!("| Max | `{max}` |\n"));
            }
            Constraint::Range(min, max) => {
                md.push_str(&format!("| Range | `{min}` - `{max}` |\n"));
            }
            Constraint::OneOf(options) => {
                md.push_str(&format!("| Allowed | `{}` |\n", options.join("`, `")));
            }
            Constraint::NotEmpty => {
                md.push_str("| Constraint | Cannot be empty |\n");
            }
            Constraint::ValidPath => {
                md.push_str("| Constraint | Must be a valid path |\n");
            }
        }

        if meta.advanced {
            md.push_str("| Advanced | Yes |\n");
        }
        if let Some(feature) = meta.requires_feature {
            md.push_str(&format!("| Requires | `{feature}` feature |\n"));
        }

        md.push('\n');

        // Use cases
        if !meta.use_cases.is_empty() {
            md.push_str("**When to use:**\n\n");
            for use_case in meta.use_cases {
                md.push_str(&format!("- {use_case}\n"));
            }
            md.push('\n');
        }

        // YAML example
        let parts: Vec<&str> = meta.key.split('.').collect();
        md.push_str("**Example:**\n\n");
        md.push_str("```yaml\n");
        self.write_yaml_example(md, &parts, meta.default);
        md.push_str("```\n\n");

        // Related configs (find others in same section)
        let related: Vec<_> = self
            .section_keys(meta.section)
            .into_iter()
            .filter(|k| *k != meta.key)
            .take(3)
            .collect();
        if !related.is_empty() {
            md.push_str(&format!("**Related:** `{}`\n\n", related.join("`, `")));
        }

        md.push_str("---\n\n");
    }

    /// Write nested YAML example
    fn write_yaml_example(&self, md: &mut String, parts: &[&str], value: &str) {
        for (i, part) in parts.iter().enumerate() {
            let indent = "  ".repeat(i);
            if i == parts.len() - 1 {
                md.push_str(&format!("{indent}{part}: {value}\n"));
            } else {
                md.push_str(&format!("{indent}{part}:\n"));
            }
        }
    }
}

impl Default for ConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global config registry singleton
static REGISTRY: std::sync::OnceLock<ConfigRegistry> = std::sync::OnceLock::new();

/// Get the global config registry
pub fn registry() -> &'static ConfigRegistry {
    REGISTRY.get_or_init(ConfigRegistry::new)
}
