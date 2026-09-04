//! Core types for code analysis and indexing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::CodeError;

/// Programming language identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Elixir,
    #[default]
    Unknown,
}

impl Language {
    /// Get file extensions for this language.
    pub fn extensions(&self) -> &[&str] {
        match self {
            Language::Rust => &["rs"],
            Language::TypeScript => &["ts", "tsx"],
            Language::JavaScript => &["js", "jsx", "mjs", "cjs"],
            Language::Python => &["py", "pyi"],
            Language::Go => &["go"],
            Language::Elixir => &["ex", "exs"],
            Language::Unknown => &[],
        }
    }

    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => Language::Rust,
            "ts" | "tsx" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            "ex" | "exs" => Language::Elixir,
            _ => Language::Unknown,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Language::Rust => write!(f, "rust"),
            Language::TypeScript => write!(f, "typescript"),
            Language::JavaScript => write!(f, "javascript"),
            Language::Python => write!(f, "python"),
            Language::Go => write!(f, "go"),
            Language::Elixir => write!(f, "elixir"),
            Language::Unknown => write!(f, "unknown"),
        }
    }
}

impl FromStr for Language {
    type Err = CodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Ok(Language::Rust),
            "typescript" | "ts" => Ok(Language::TypeScript),
            "javascript" | "js" => Ok(Language::JavaScript),
            "python" | "py" => Ok(Language::Python),
            "go" | "golang" => Ok(Language::Go),
            "elixir" | "ex" | "exs" => Ok(Language::Elixir),
            "unknown" => Ok(Language::Unknown),
            _ => Err(CodeError::Parse(format!("unknown language: {s}"))),
        }
    }
}

/// Type of code symbol (function, class, struct, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    #[default]
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Module,
    Constant,
    Variable,
    Type,
    Import,
    Impl,
    Macro,
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "function"),
            SymbolKind::Method => write!(f, "method"),
            SymbolKind::Class => write!(f, "class"),
            SymbolKind::Struct => write!(f, "struct"),
            SymbolKind::Enum => write!(f, "enum"),
            SymbolKind::Trait => write!(f, "trait"),
            SymbolKind::Interface => write!(f, "interface"),
            SymbolKind::Module => write!(f, "module"),
            SymbolKind::Constant => write!(f, "constant"),
            SymbolKind::Variable => write!(f, "variable"),
            SymbolKind::Type => write!(f, "type"),
            SymbolKind::Import => write!(f, "import"),
            SymbolKind::Impl => write!(f, "impl"),
            SymbolKind::Macro => write!(f, "macro"),
        }
    }
}

impl SymbolKind {
    /// Every variant, in declaration order.
    ///
    /// Load-bearing for callers that must enumerate kinds *outside* Rust — the
    /// vector-coverage join builds its SQL `IN (...)` list from
    /// [`Self::embeddable_kind_names`], and a kind missing from this array
    /// would silently drop real symbols out of the denominator. The
    /// exhaustiveness test below fails to compile when a variant is added
    /// without being listed here.
    pub const ALL: &'static [SymbolKind] = &[
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Class,
        SymbolKind::Struct,
        SymbolKind::Enum,
        SymbolKind::Trait,
        SymbolKind::Interface,
        SymbolKind::Module,
        SymbolKind::Constant,
        SymbolKind::Variable,
        SymbolKind::Type,
        SymbolKind::Import,
        SymbolKind::Impl,
        SymbolKind::Macro,
    ];

    /// Stored `code_symbols.kind` text for every kind the semantic drain embeds.
    ///
    /// The strings are the [`Display`](fmt::Display) form, which is exactly what
    /// the code store writes, so a SQL comparison against this list matches the
    /// same rows the drain would queue.
    pub fn embeddable_kind_names() -> Vec<String> {
        Self::ALL
            .iter()
            .filter(|kind| kind.should_embed())
            .map(|kind| kind.to_string())
            .collect()
    }

    /// Returns true if this symbol kind is valuable enough to generate embeddings for.
    /// Low-value symbols (imports, variables, constants) are still indexed in BM25
    /// for keyword search, but skip the expensive embedding step.
    pub fn should_embed(&self) -> bool {
        match self {
            // High-value: core logic and API surface
            SymbolKind::Function => true,
            SymbolKind::Method => true,
            SymbolKind::Class => true,
            SymbolKind::Struct => true,
            SymbolKind::Enum => true,
            SymbolKind::Trait => true,
            SymbolKind::Interface => true,
            SymbolKind::Type => true,
            SymbolKind::Macro => true,
            // Low-value: skip embedding (still in BM25)
            SymbolKind::Import => false,
            SymbolKind::Variable => false,
            SymbolKind::Constant => false,
            SymbolKind::Module => false,
            SymbolKind::Impl => false,
        }
    }
}

impl FromStr for SymbolKind {
    type Err = CodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "function" | "func" | "fn" => Ok(SymbolKind::Function),
            "method" => Ok(SymbolKind::Method),
            "class" => Ok(SymbolKind::Class),
            "struct" => Ok(SymbolKind::Struct),
            "enum" => Ok(SymbolKind::Enum),
            "trait" => Ok(SymbolKind::Trait),
            "interface" => Ok(SymbolKind::Interface),
            "module" | "mod" => Ok(SymbolKind::Module),
            "constant" | "const" => Ok(SymbolKind::Constant),
            "variable" | "var" | "let" => Ok(SymbolKind::Variable),
            "type" => Ok(SymbolKind::Type),
            "import" | "use" => Ok(SymbolKind::Import),
            "impl" => Ok(SymbolKind::Impl),
            "macro" | "defmacro" => Ok(SymbolKind::Macro),
            _ => Err(CodeError::Parse(format!("unknown symbol kind: {s}"))),
        }
    }
}

/// Relationship type between code symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeRelationType {
    #[default]
    Imports,
    Calls,
    Extends,
    Implements,
    Contains,
    References,
    Tests,
}

impl fmt::Display for CodeRelationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodeRelationType::Imports => write!(f, "imports"),
            CodeRelationType::Calls => write!(f, "calls"),
            CodeRelationType::Extends => write!(f, "extends"),
            CodeRelationType::Implements => write!(f, "implements"),
            CodeRelationType::Contains => write!(f, "contains"),
            CodeRelationType::References => write!(f, "references"),
            CodeRelationType::Tests => write!(f, "tests"),
        }
    }
}

impl FromStr for CodeRelationType {
    type Err = CodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "imports" | "import" => Ok(CodeRelationType::Imports),
            "calls" | "call" => Ok(CodeRelationType::Calls),
            "extends" | "extend" => Ok(CodeRelationType::Extends),
            "implements" | "implement" => Ok(CodeRelationType::Implements),
            "contains" | "contain" => Ok(CodeRelationType::Contains),
            "references" | "reference" | "ref" => Ok(CodeRelationType::References),
            "tests" | "test" => Ok(CodeRelationType::Tests),
            _ => Err(CodeError::Parse(format!("unknown relation type: {s}"))),
        }
    }
}

/// Link type between code and CAS memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeMemoryLinkType {
    #[default]
    Documents,
    BugReport,
    DesignDecision,
    Learning,
    Reference,
}

impl fmt::Display for CodeMemoryLinkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodeMemoryLinkType::Documents => write!(f, "documents"),
            CodeMemoryLinkType::BugReport => write!(f, "bug_report"),
            CodeMemoryLinkType::DesignDecision => write!(f, "design_decision"),
            CodeMemoryLinkType::Learning => write!(f, "learning"),
            CodeMemoryLinkType::Reference => write!(f, "reference"),
        }
    }
}

impl FromStr for CodeMemoryLinkType {
    type Err = CodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "documents" | "doc" => Ok(CodeMemoryLinkType::Documents),
            "bug_report" | "bug" => Ok(CodeMemoryLinkType::BugReport),
            "design_decision" | "design" => Ok(CodeMemoryLinkType::DesignDecision),
            "learning" | "learn" => Ok(CodeMemoryLinkType::Learning),
            "reference" | "ref" => Ok(CodeMemoryLinkType::Reference),
            _ => Err(CodeError::Parse(format!("unknown link type: {s}"))),
        }
    }
}

/// A tracked source code file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFile {
    /// Unique identifier (e.g., "file-a1b2c3d4")
    pub id: String,

    /// Path relative to repository root
    pub path: String,

    /// Repository/project identifier
    pub repository: String,

    /// Programming language
    pub language: Language,

    /// File size in bytes
    pub size: usize,

    /// Number of lines
    pub line_count: usize,

    /// Git commit hash when indexed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,

    /// SHA-256 hash of content for change detection
    pub content_hash: String,

    /// When first indexed
    pub created: DateTime<Utc>,

    /// When last updated
    pub updated: DateTime<Utc>,

    /// Scope (global or project)
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "project".to_string()
}

impl Default for CodeFile {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: String::new(),
            path: String::new(),
            repository: String::new(),
            language: Language::Unknown,
            size: 0,
            line_count: 0,
            commit_hash: None,
            content_hash: String::new(),
            created: now,
            updated: now,
            scope: default_scope(),
        }
    }
}

/// A code symbol (function, class, struct, etc.) extracted from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSymbol {
    /// Unique identifier (e.g., "sym-a1b2c3d4")
    pub id: String,

    /// Fully qualified name (e.g., "cas_cli::search::hybrid::HybridSearch::search")
    pub qualified_name: String,

    /// Short name (e.g., "search")
    pub name: String,

    /// Type of symbol
    pub kind: SymbolKind,

    /// Programming language
    pub language: Language,

    /// File path relative to repository root
    pub file_path: String,

    /// File ID (foreign key to code_files)
    pub file_id: String,

    /// Line range (start, end) - 1-indexed
    pub line_start: usize,
    pub line_end: usize,

    /// The actual source code
    pub source: String,

    /// Documentation/docstring if present
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,

    /// Signature (for functions/methods)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Parent symbol ID (for methods in impls, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Repository/project identifier
    pub repository: String,

    /// Git commit hash when indexed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,

    /// When first indexed
    pub created: DateTime<Utc>,

    /// When last updated
    pub updated: DateTime<Utc>,

    /// SHA-256 hash of source for change detection
    pub content_hash: String,

    /// Scope (global or project)
    #[serde(default = "default_scope")]
    pub scope: String,
}

impl Default for CodeSymbol {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            id: String::new(),
            qualified_name: String::new(),
            name: String::new(),
            kind: SymbolKind::Function,
            language: Language::Unknown,
            file_path: String::new(),
            file_id: String::new(),
            line_start: 0,
            line_end: 0,
            source: String::new(),
            documentation: None,
            signature: None,
            parent_id: None,
            repository: String::new(),
            commit_hash: None,
            created: now,
            updated: now,
            content_hash: String::new(),
            scope: default_scope(),
        }
    }
}

/// A relationship between code symbols (imports, calls, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeRelationship {
    /// Unique identifier
    pub id: String,

    /// Source symbol ID
    pub source_id: String,

    /// Target symbol ID
    pub target_id: String,

    /// Type of relationship
    pub relation_type: CodeRelationType,

    /// Relationship weight/strength (default 1.0)
    #[serde(default = "default_weight")]
    pub weight: f32,

    /// When created
    pub created: DateTime<Utc>,
}

fn default_weight() -> f32 {
    1.0
}

impl Default for CodeRelationship {
    fn default() -> Self {
        Self {
            id: String::new(),
            source_id: String::new(),
            target_id: String::new(),
            relation_type: CodeRelationType::References,
            weight: default_weight(),
            created: Utc::now(),
        }
    }
}

/// A link between a code symbol and a CAS memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeMemoryLink {
    /// Code symbol or file ID
    pub code_id: String,

    /// CAS entry/memory ID
    pub entry_id: String,

    /// Type of link
    pub link_type: CodeMemoryLinkType,

    /// Confidence score (0.0 - 1.0)
    #[serde(default = "default_confidence")]
    pub confidence: f32,

    /// When the link was created
    pub created: DateTime<Utc>,
}

fn default_confidence() -> f32 {
    0.8
}

impl Default for CodeMemoryLink {
    fn default() -> Self {
        Self {
            code_id: String::new(),
            entry_id: String::new(),
            link_type: CodeMemoryLinkType::Reference,
            confidence: default_confidence(),
            created: Utc::now(),
        }
    }
}

/// Import statement extracted from source code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Import {
    /// Module/package path being imported
    pub module_path: String,

    /// Specific items imported (empty for module-level import)
    pub items: Vec<String>,

    /// Line number where import appears
    pub line: usize,

    /// Whether this is a re-export (pub use in Rust)
    #[serde(default)]
    pub is_reexport: bool,
}

/// Result of parsing a source file.
#[derive(Debug, Clone, Default)]
pub struct ParseResult {
    /// Extracted symbols
    pub symbols: Vec<CodeSymbol>,

    /// Extracted imports
    pub imports: Vec<Import>,

    /// Parse errors (non-fatal)
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::types::*;

    #[test]
    fn test_language_from_extension() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("tsx"), Language::TypeScript);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("go"), Language::Go);
        assert_eq!(Language::from_extension("txt"), Language::Unknown);
    }

    #[test]
    fn test_language_roundtrip() {
        for lang in [
            Language::Rust,
            Language::TypeScript,
            Language::Python,
            Language::Go,
        ] {
            let s = lang.to_string();
            let parsed: Language = s.parse().unwrap();
            assert_eq!(lang, parsed);
        }
    }

    /// `SymbolKind::ALL` is what turns Rust's `should_embed` into a SQL kind
    /// list. A variant missing from it would quietly shrink the vector-coverage
    /// denominator, so assert it holds every kind exactly once. The match makes
    /// the omission a compile error rather than a silent undercount.
    #[test]
    fn all_symbol_kinds_are_listed_once_and_round_trip() {
        for kind in SymbolKind::ALL {
            match kind {
                SymbolKind::Function
                | SymbolKind::Method
                | SymbolKind::Class
                | SymbolKind::Struct
                | SymbolKind::Enum
                | SymbolKind::Trait
                | SymbolKind::Interface
                | SymbolKind::Module
                | SymbolKind::Constant
                | SymbolKind::Variable
                | SymbolKind::Type
                | SymbolKind::Import
                | SymbolKind::Impl
                | SymbolKind::Macro => {}
            }
            let parsed: SymbolKind = kind.to_string().parse().unwrap();
            assert_eq!(*kind, parsed);
        }
        let unique: std::collections::HashSet<_> = SymbolKind::ALL.iter().collect();
        assert_eq!(unique.len(), SymbolKind::ALL.len());
        assert_eq!(SymbolKind::ALL.len(), 14);
    }

    #[test]
    fn embeddable_kind_names_are_the_stored_kind_text() {
        let names = SymbolKind::embeddable_kind_names();
        assert!(names.contains(&"function".to_string()));
        assert!(names.contains(&"macro".to_string()));
        assert!(!names.contains(&"import".to_string()));
        assert_eq!(
            names.len(),
            SymbolKind::ALL
                .iter()
                .filter(|kind| kind.should_embed())
                .count()
        );
    }

    #[test]
    fn test_symbol_kind_roundtrip() {
        for kind in [
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Class,
            SymbolKind::Struct,
        ] {
            let s = kind.to_string();
            let parsed: SymbolKind = s.parse().unwrap();
            assert_eq!(kind, parsed);
        }
    }
}
