//! Rust language parser using tree-sitter.
#![allow(
    clippy::needless_borrow,
    clippy::to_string_in_format_args,
    clippy::too_many_arguments
)]

mod imports;
mod symbol_extract;
mod tests;
mod walker;

use std::path::Path;

use sha2::{Digest, Sha256};
use tree_sitter::{Node, Parser, Tree};

use crate::error::{CodeError, Result};
use crate::parser::LanguageParser;
use crate::types::{Import, Language, ParseResult};

/// Rust language parser.
pub struct RustParser {
    parser: Parser,
}

impl RustParser {
    /// Create a new Rust parser.
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|e| CodeError::TreeSitter(format!("failed to set language: {e}")))?;

        Ok(Self { parser })
    }

    /// Generate a content hash for a code snippet.
    fn content_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Generate a unique symbol ID.
    fn generate_id() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);

        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_millis() as u32)
            .unwrap_or(0);

        format!("sym-{:04x}{:04x}", timestamp & 0xFFFF, count & 0xFFFF)
    }

    /// Extract text from a node.
    fn node_text<'a>(&self, node: &Node, source: &'a [u8]) -> &'a str {
        node.utf8_text(source).unwrap_or("")
    }

    /// Extract documentation comment above a node.
    fn extract_doc_comment(&self, node: &Node, source: &[u8]) -> Option<String> {
        let mut doc_lines = Vec::new();
        let mut sibling = node.prev_sibling();

        while let Some(prev) = sibling {
            if prev.kind() == "line_comment" {
                let text = self.node_text(&prev, source);
                if text.starts_with("///") || text.starts_with("//!") {
                    doc_lines.push(text.trim_start_matches('/').trim().to_string());
                } else {
                    break;
                }
            } else if prev.kind() == "block_comment" {
                let text = self.node_text(&prev, source);
                if text.starts_with("/**") || text.starts_with("/*!") {
                    doc_lines.push(
                        text.trim_start_matches("/*")
                            .trim_end_matches("*/")
                            .trim()
                            .to_string(),
                    );
                }
                break;
            } else if prev.kind() != "attribute_item" {
                break;
            }
            sibling = prev.prev_sibling();
        }

        if doc_lines.is_empty() {
            None
        } else {
            doc_lines.reverse();
            Some(doc_lines.join("\n"))
        }
    }

    /// Parse source code into an AST.
    fn parse(&mut self, source: &str) -> Result<Tree> {
        self.parser
            .parse(source, None)
            .ok_or_else(|| CodeError::TreeSitter("failed to parse source".to_string()))
    }

    /// Top-level external modules with an explicit `#[path]`, as
    /// `(module name, relative file path)`. Comments, example strings, inline
    /// modules and ambiguous or unsupported path attributes cannot claim a file.
    pub fn explicit_external_modules(&mut self, source: &str) -> Result<Vec<(String, String)>> {
        let tree = self.parse(source)?;
        let root = tree.root_node();
        if root.has_error() {
            return Err(CodeError::Parse("invalid module source".into()));
        }
        let mut modules = Vec::new();
        let mut cursor = root.walk();
        for module in root.named_children(&mut cursor) {
            if module.kind() != "mod_item" || module.child_by_field_name("body").is_some() {
                continue;
            }
            let Some(name) = module.child_by_field_name("name") else {
                continue;
            };
            let mut paths = Vec::new();
            let mut sibling = module.prev_named_sibling();
            while let Some(node) = sibling {
                match node.kind() {
                    "line_comment" | "block_comment" => {}
                    "attribute_item" => {
                        if let Some(attribute) = node.named_child(0)
                            && attribute.named_child(0).is_some_and(|name| {
                                self.node_text(&name, source.as_bytes()) == "path"
                            })
                        {
                            let path =
                                attribute.child_by_field_name("value").and_then(
                                    |value| match value.kind() {
                                        "string_literal" => serde_json::from_str::<String>(
                                            self.node_text(&value, source.as_bytes()),
                                        )
                                        .ok(),
                                        "raw_string_literal" => value.named_child(0).map(|text| {
                                            self.node_text(&text, source.as_bytes()).to_string()
                                        }),
                                        _ => None,
                                    },
                                );
                            // Retain invalid attributes too: two #[path] values
                            // must not become unambiguous by dropping one.
                            paths.push(path);
                        }
                    }
                    _ => break,
                }
                sibling = node.prev_named_sibling();
            }
            if paths.len() == 1
                && let Some(path) = paths.pop().flatten()
            {
                modules.push((self.node_text(&name, source.as_bytes()).to_string(), path));
            }
        }
        Ok(modules)
    }
}

impl LanguageParser for RustParser {
    fn parse_file(&mut self, path: &Path, content: &str, repository: &str) -> Result<ParseResult> {
        let tree = self.parse(content)?;
        let root = tree.root_node();

        let file_path = path.to_string_lossy().to_string();
        let file_id = format!("file-{}", Self::content_hash(&file_path)[..8].to_string());

        let mut symbols = Vec::new();
        let mut imports = Vec::new();

        self.walk_tree(
            &root,
            content.as_bytes(),
            &file_path,
            &file_id,
            repository,
            "",
            &mut symbols,
            &mut imports,
        );

        let mut errors = Vec::new();
        if root.has_error() {
            errors.push("source contains parse errors".to_string());
        }

        Ok(ParseResult {
            symbols,
            imports,
            errors,
        })
    }

    fn extract_imports(&mut self, content: &str) -> Result<Vec<Import>> {
        let tree = self.parse(content)?;
        let root = tree.root_node();

        let mut imports = Vec::new();
        let mut cursor = root.walk();

        for child in root.children(&mut cursor) {
            if child.kind() == "use_declaration" {
                if let Some(import) = self.extract_use_statement(&child, content.as_bytes()) {
                    imports.push(import);
                }
            }
        }

        Ok(imports)
    }

    fn extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn language(&self) -> Language {
        Language::Rust
    }
}
