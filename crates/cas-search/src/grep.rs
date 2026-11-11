//! Grep search using ripgrep libraries
//!
//! Provides file content search using the same libraries that power ripgrep:
//! - grep-searcher for line-oriented search
//! - grep-regex for regex matching
//! - ignore for gitignore-aware file walking
//!
//! This module is fully generic with no CAS dependencies.

use std::path::{Path, PathBuf};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch,
};
use ignore::WalkBuilder;

use crate::error::{Result, SearchError};

/// A single grep match result
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// File path where match was found (relative to search root)
    pub file_path: String,
    /// Line number (1-indexed)
    pub line_number: u64,
    /// The matching line content
    pub line_content: String,
    /// Context lines before the match
    pub before_context: Vec<String>,
    /// Context lines after the match
    pub after_context: Vec<String>,
}

/// Options for grep search
#[derive(Debug, Clone)]
pub struct GrepOptions {
    /// Regex pattern to search for
    pub pattern: String,
    /// File glob pattern (e.g., "*.rs", "src/**/*.ts")
    pub glob: Option<String>,
    /// Lines of context before match
    pub before_context: usize,
    /// Lines of context after match
    pub after_context: usize,
    /// Case insensitive search
    pub case_insensitive: bool,
    /// Maximum results to return
    pub limit: usize,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            glob: None,
            before_context: 0,
            after_context: 0,
            case_insensitive: false,
            limit: 100,
        }
    }
}

/// Custom sink that captures matches with context
struct ContextSink {
    file_path: String,
    matches: Vec<GrepMatch>,
    before_buffer: Vec<String>,
    current_match: Option<GrepMatch>,
    after_remaining: usize,
    after_context_count: usize,
    limit: usize,
}

impl ContextSink {
    fn new(file_path: String, before_context: usize, after_context: usize, limit: usize) -> Self {
        Self {
            file_path,
            matches: Vec::new(),
            before_buffer: Vec::with_capacity(before_context),
            current_match: None,
            after_remaining: 0,
            after_context_count: after_context,
            limit,
        }
    }

    fn finalize(mut self) -> Vec<GrepMatch> {
        if let Some(m) = self.current_match.take() {
            self.matches.push(m);
        }
        self.matches
    }
}

impl Sink for ContextSink {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        mat: &SinkMatch<'_>,
    ) -> std::result::Result<bool, Self::Error> {
        // Save previous match if any
        if let Some(m) = self.current_match.take() {
            self.matches.push(m);
        }

        // Check limit AFTER saving previous match
        if self.matches.len() >= self.limit {
            return Ok(false);
        }

        // Create new match with before context
        let line_content = String::from_utf8_lossy(mat.bytes()).trim_end().to_string();
        self.current_match = Some(GrepMatch {
            file_path: self.file_path.clone(),
            line_number: mat.line_number().unwrap_or(0),
            line_content,
            before_context: self.before_buffer.drain(..).collect(),
            after_context: Vec::new(),
        });
        self.after_remaining = self.after_context_count;

        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> std::result::Result<bool, Self::Error> {
        let line = String::from_utf8_lossy(ctx.bytes()).trim_end().to_string();

        match ctx.kind() {
            SinkContextKind::Before => {
                if self.before_buffer.len() >= self.before_buffer.capacity()
                    && !self.before_buffer.is_empty()
                {
                    self.before_buffer.remove(0);
                }
                self.before_buffer.push(line);
            }
            SinkContextKind::After => {
                if let Some(ref mut m) = self.current_match {
                    if self.after_remaining > 0 {
                        m.after_context.push(line);
                        self.after_remaining -= 1;
                    }
                }
            }
            SinkContextKind::Other => {}
        }
        Ok(true)
    }
}

/// Grep search implementation using ripgrep libraries
pub struct GrepSearch {
    root: PathBuf,
}

impl GrepSearch {
    /// Create a new grep search rooted at the given path
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Get the search root path
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Search for matches in files under the root directory
    // gate-audit[override_builder]: selector ripgrep builder for matching semantics, not a factory policy exception.
    pub fn search(&self, opts: &GrepOptions) -> Result<Vec<GrepMatch>> {
        if opts.pattern.is_empty() {
            return Err(SearchError::Query("grep pattern cannot be empty".into()));
        }

        // Build the regex matcher
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(opts.case_insensitive)
            .build(&opts.pattern)
            .map_err(|e| SearchError::Query(format!("Invalid regex pattern: {e}")))?;

        // Build the searcher with context support
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .before_context(opts.before_context)
            .after_context(opts.after_context)
            .build();

        let mut results = Vec::new();
        let limit = opts.limit;

        // Build file walker with gitignore support
        let mut walker_builder = WalkBuilder::new(&self.root);
        walker_builder
            .hidden(false)
            .ignore(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .add_custom_ignore_filename(".casignore");

        // Add glob filter if specified
        if let Some(ref glob) = opts.glob {
            if glob.contains('/') || glob.contains("**") {
                // Path pattern - use overrides
                let mut override_builder = ignore::overrides::OverrideBuilder::new(&self.root);
                override_builder.add(glob).ok();
                if let Ok(overrides) = override_builder.build() {
                    walker_builder.overrides(overrides);
                }
            } else {
                // Simple extension pattern - use types
                let mut types_builder = ignore::types::TypesBuilder::new();
                types_builder.add("custom", glob).ok();
                types_builder.select("custom");
                if let Ok(types) = types_builder.build() {
                    walker_builder.types(types);
                }
            }
        }

        let walker = walker_builder.build();

        // Search each file
        for entry in walker.filter_map(|e| e.ok()) {
            if results.len() >= limit {
                break;
            }

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let file_path = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let remaining = limit.saturating_sub(results.len());
            let mut sink = ContextSink::new(
                file_path,
                opts.before_context,
                opts.after_context,
                remaining,
            );

            let search_result = searcher.search_path(&matcher, path, &mut sink);

            if search_result.is_ok() {
                results.extend(sink.finalize());
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use crate::grep::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_grep_simple_pattern() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        let grep = GrepSearch::new(dir.path());
        let results = grep
            .search(&GrepOptions {
                pattern: "println".to_string(),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_number, 2);
        assert!(results[0].line_content.contains("println"));
    }

    #[test]
    fn test_grep_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "Hello World\nhello world\nHELLO WORLD\n").unwrap();

        let grep = GrepSearch::new(dir.path());
        let results = grep
            .search(&GrepOptions {
                pattern: "hello".to_string(),
                case_insensitive: true,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_grep_regex_pattern() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn foo() {}\nfn bar() {}\nfn baz() {}\n").unwrap();

        let grep = GrepSearch::new(dir.path());
        let results = grep
            .search(&GrepOptions {
                pattern: r"fn\s+ba".to_string(),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_grep_limit() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "match\nmatch\nmatch\nmatch\nmatch\n").unwrap();

        let grep = GrepSearch::new(dir.path());
        let results = grep
            .search(&GrepOptions {
                pattern: "match".to_string(),
                limit: 3,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_grep_path_glob() {
        let dir = TempDir::new().unwrap();

        let src_dir = dir.path().join("src");
        let lib_dir = dir.path().join("lib");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        fs::write(src_dir.join("foo.rs"), "fn foo() {}\n").unwrap();
        fs::write(lib_dir.join("bar.rs"), "fn bar() {}\n").unwrap();

        let grep = GrepSearch::new(dir.path());

        let results = grep
            .search(&GrepOptions {
                pattern: "fn".to_string(),
                glob: Some("src/**/*.rs".to_string()),
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].file_path.contains("foo"));
    }

    #[test]
    fn test_grep_with_context() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "line 1\nline 2\nMATCH\nline 4\nline 5\n").unwrap();

        let grep = GrepSearch::new(dir.path());
        let results = grep
            .search(&GrepOptions {
                pattern: "MATCH".to_string(),
                before_context: 2,
                after_context: 2,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].line_content, "MATCH");
        assert_eq!(results[0].before_context, vec!["line 1", "line 2"]);
        assert_eq!(results[0].after_context, vec!["line 4", "line 5"]);
    }

    #[test]
    fn test_grep_empty_pattern_error() {
        let dir = TempDir::new().unwrap();
        let grep = GrepSearch::new(dir.path());
        let result = grep.search(&GrepOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_grep_invalid_regex_error() {
        let dir = TempDir::new().unwrap();
        let grep = GrepSearch::new(dir.path());
        let result = grep.search(&GrepOptions {
            pattern: "[invalid".to_string(),
            ..Default::default()
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_grep_options_default() {
        let opts = GrepOptions::default();
        assert!(opts.pattern.is_empty());
        assert!(opts.glob.is_none());
        assert_eq!(opts.before_context, 0);
        assert_eq!(opts.after_context, 0);
        assert!(!opts.case_insensitive);
        assert_eq!(opts.limit, 100);
    }
}
