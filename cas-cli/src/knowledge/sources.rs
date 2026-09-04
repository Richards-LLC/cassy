//! Source selection for knowledge distillation (EPIC cas-7d31 / cas-c9be).
//!
//! The distillable source set is deliberately narrow and deterministic:
//!
//! - prose docs: `README*`, `CLAUDE.md`, `AGENTS.md`, any `*.md` under the repo
//!   that is not inside an ignored directory,
//! - key configs: `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`,
//!   `Makefile`, `Dockerfile`, `docker-compose.y{a,}ml`,
//! - code-derived module summaries synthesized from the indexed `code_symbols`
//!   table, so subsystems with no prose still get a page.
//!
//! Every source is reduced to a [`LoadedSource`] carrying its content and the
//! BLAKE3 hash the ledger compares against. Code module summaries are *virtual*
//! sources: their `path` is `code://<module>` and their hash is the hash of the
//! synthesized summary text, so a module whose symbols did not move is skipped
//! by exactly the same ledger short-circuit as an unchanged file.

use std::collections::BTreeMap;
use std::path::Path;

use cas_store::{DiskSource, blake3_hex};

/// Files larger than this are never distilled — a vendored blob or a generated
/// lockfile is cost without signal.
pub const MAX_SOURCE_BYTES: u64 = 512 * 1024;

/// Prefix marking a synthesized (non-file) source path.
pub const CODE_MODULE_SCHEME: &str = "code://";

/// Directory names never walked, regardless of gitignore state.
const SKIP_DIRS: &[&str] = &[
    ".cas",
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    ".venv",
    "__pycache__",
    ".next",
];

/// Config files worth distilling (build/runtime shape of the project).
const CONFIG_FILES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Makefile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// What kind of thing a source is — drives the default page type hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Human-written prose (markdown).
    Doc,
    /// Build/runtime configuration.
    Config,
    /// Synthesized summary of an indexed code module.
    CodeModule,
}

impl SourceKind {
    /// Page-type hint handed to the distiller for sources of this kind.
    pub fn page_type_hint(self) -> &'static str {
        match self {
            Self::Doc => "guide",
            Self::Config => "configuration",
            Self::CodeModule => "subsystem",
        }
    }
}

/// A source file (or synthesized module) with its content and ledger identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSource {
    /// Ledger key: repo-relative path, or `code://<module>` for a virtual source.
    pub path: String,
    pub content: String,
    pub blake3: String,
    pub size: u64,
    pub kind: SourceKind,
}

impl LoadedSource {
    /// Build a source from already-materialized content.
    pub fn from_content(
        path: impl Into<String>,
        content: impl Into<String>,
        kind: SourceKind,
    ) -> Self {
        let content = content.into();
        let bytes = content.as_bytes();
        Self {
            path: path.into(),
            blake3: blake3_hex(bytes),
            size: bytes.len() as u64,
            content,
            kind,
        }
    }

    /// The ledger's view of this source.
    pub fn as_disk_source(&self) -> DiskSource {
        DiskSource {
            file_path: self.path.clone(),
            blake3: self.blake3.clone(),
            size: self.size,
        }
    }
}

/// Ledger view of a whole source set, in the same order.
pub fn disk_sources(loaded: &[LoadedSource]) -> Vec<DiskSource> {
    loaded.iter().map(LoadedSource::as_disk_source).collect()
}

/// Is this repo-relative path a prose doc we distill?
pub fn is_doc_source(rel_path: &str) -> bool {
    let name = file_name_of(rel_path);
    if name.eq_ignore_ascii_case("README") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".mdx") || lower.starts_with("readme.")
}

/// Is this repo-relative path a config file we distill?
pub fn is_config_source(rel_path: &str) -> bool {
    let name = file_name_of(rel_path);
    CONFIG_FILES.iter().any(|candidate| *candidate == name)
}

fn file_name_of(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

/// Classify a repo-relative path, or `None` if it is not a distillable source.
pub fn classify_path(rel_path: &str) -> Option<SourceKind> {
    if rel_path
        .split('/')
        .any(|component| SKIP_DIRS.contains(&component))
    {
        return None;
    }
    if is_doc_source(rel_path) {
        Some(SourceKind::Doc)
    } else if is_config_source(rel_path) {
        Some(SourceKind::Config)
    } else {
        None
    }
}

/// A distillable file the collector could not turn into text, with the reason.
///
/// cas-c736: these used to vanish. A UTF-16 `README.md` was dropped by a bare
/// `let Ok(...) else { continue }`, so the wiki simply had no page for it and
/// no line anywhere said why — the failure mode that is hardest to notice,
/// because the output looks complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedSource {
    /// Project-relative path, the same identity a [`LoadedSource`] carries.
    pub path: String,
    /// Operator-facing reason, already phrased to stand alone in a report line.
    pub reason: String,
}

/// The result of a source walk: what loaded, and what was passed over by name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceScan {
    pub sources: Vec<LoadedSource>,
    pub skipped: Vec<SkippedSource>,
}

impl SourceScan {
    /// Report lines for the skipped files, ready to append to a pass's notes.
    pub fn skip_notes(&self) -> Vec<String> {
        self.skipped
            .iter()
            .map(|skip| format!("source skipped: {} ({})", skip.path, skip.reason))
            .collect()
    }
}

/// Walk `project_root` and load every distillable file, sorted by path so a
/// pass is deterministic.
///
/// Oversized files are skipped by design (a vendored blob is cost without
/// signal) and stay unreported. Files that cannot be read or decoded are
/// reported by name in [`SourceScan::skipped`]: they were *meant* to be
/// distilled, so their absence from the wiki is a fact the pass owes its
/// operator.
pub fn scan_file_sources(project_root: &Path) -> SourceScan {
    let mut found: BTreeMap<String, LoadedSource> = BTreeMap::new();
    let mut skipped: BTreeMap<String, SkippedSource> = BTreeMap::new();

    let walker = ignore::WalkBuilder::new(project_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .parents(false)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(project_root) else {
            continue;
        };
        let rel_path = rel.to_string_lossy().replace('\\', "/");
        let Some(kind) = classify_path(&rel_path) else {
            continue;
        };
        let too_big = entry
            .metadata()
            .map(|meta| meta.len() > MAX_SOURCE_BYTES)
            .unwrap_or(true);
        if too_big {
            continue;
        }
        // Bytes then decode, not `read_to_string`: a BOM-marked UTF-16 doc is
        // ordinary prose, and a UTF-8 BOM left on the front glues itself to the
        // first markdown heading the distiller reads (GH #698, cas-c736).
        let content = match std::fs::read(entry.path()) {
            Ok(bytes) => match crate::daemon::source_text::decode_source(&bytes) {
                Ok(content) => content,
                Err(reason) => {
                    skipped.insert(
                        rel_path.clone(),
                        SkippedSource {
                            path: rel_path,
                            reason: reason.as_str().to_string(),
                        },
                    );
                    continue;
                }
            },
            Err(error) => {
                skipped.insert(
                    rel_path.clone(),
                    SkippedSource {
                        path: rel_path,
                        reason: error.to_string(),
                    },
                );
                continue;
            }
        };
        found.insert(
            rel_path.clone(),
            LoadedSource::from_content(rel_path, content, kind),
        );
    }

    SourceScan {
        sources: found.into_values().collect(),
        skipped: skipped.into_values().collect(),
    }
}

/// [`scan_file_sources`] for callers that only need the loaded set.
pub fn collect_file_sources(project_root: &Path) -> Vec<LoadedSource> {
    scan_file_sources(project_root).sources
}

/// The slice of an indexed symbol that a module summary needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolLite {
    pub file_path: String,
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub doc: Option<String>,
}

/// Max symbols listed per synthesized module summary — the summary is a seed
/// for the LLM, not an API reference.
const MAX_SYMBOLS_PER_MODULE: usize = 60;

/// Group indexed symbols into per-directory module summaries.
///
/// Pure and deterministic: same symbols in any order produce the same set of
/// sources with the same hashes, which is what lets the ledger skip unchanged
/// modules without any LLM call.
pub fn build_module_sources(symbols: &[SymbolLite], project_root: &Path) -> Vec<LoadedSource> {
    let root = project_root.to_string_lossy().replace('\\', "/");
    let mut by_module: BTreeMap<String, Vec<&SymbolLite>> = BTreeMap::new();

    for symbol in symbols {
        let normalized = symbol.file_path.replace('\\', "/");
        let rel = normalized
            .strip_prefix(&root)
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(&normalized)
            .to_string();
        if rel
            .split('/')
            .any(|component| SKIP_DIRS.contains(&component))
        {
            continue;
        }
        let module = match rel.rfind('/') {
            Some(index) => rel[..index].to_string(),
            None => continue, // top-level loose file: covered by docs/configs
        };
        if module.is_empty() {
            continue;
        }
        by_module.entry(module).or_default().push(symbol);
    }

    by_module
        .into_iter()
        .map(|(module, mut members)| {
            members.sort_by(|a, b| {
                a.file_path
                    .cmp(&b.file_path)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.kind.cmp(&b.kind))
            });
            members.dedup_by(|a, b| {
                a.name == b.name && a.kind == b.kind && a.file_path == b.file_path
            });

            let mut text = format!("# Module `{module}`\n\nIndexed symbols:\n\n");
            for symbol in members.iter().take(MAX_SYMBOLS_PER_MODULE) {
                text.push_str(&format!("- `{}` ({})", symbol.name, symbol.kind));
                if let Some(signature) = symbol.signature.as_deref().filter(|s| !s.is_empty()) {
                    text.push_str(&format!(" — `{}`", first_line(signature)));
                }
                if let Some(doc) = symbol.doc.as_deref().filter(|s| !s.is_empty()) {
                    text.push_str(&format!("\n  {}", first_line(doc)));
                }
                text.push('\n');
            }
            if members.len() > MAX_SYMBOLS_PER_MODULE {
                text.push_str(&format!(
                    "\n({} more symbols not listed.)\n",
                    members.len() - MAX_SYMBOLS_PER_MODULE
                ));
            }

            LoadedSource::from_content(
                format!("{CODE_MODULE_SCHEME}{module}"),
                text,
                SourceKind::CodeModule,
            )
        })
        .collect()
}

fn first_line(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn symbol(file: &str, name: &str, kind: &str) -> SymbolLite {
        SymbolLite {
            file_path: file.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            signature: None,
            doc: None,
        }
    }

    #[test]
    fn docs_and_configs_are_recognized_and_junk_is_not() {
        assert_eq!(classify_path("README.md"), Some(SourceKind::Doc));
        assert_eq!(classify_path("docs/guide.md"), Some(SourceKind::Doc));
        assert_eq!(classify_path("Cargo.toml"), Some(SourceKind::Config));
        assert_eq!(classify_path("src/main.rs"), None);
        assert_eq!(classify_path("target/doc/index.md"), None);
        assert_eq!(classify_path("node_modules/pkg/README.md"), None);
        assert_eq!(classify_path(".cas/knowledge/a/b.md"), None);
    }

    #[test]
    fn loading_a_source_hashes_its_content() {
        let source = LoadedSource::from_content("README.md", "hello", SourceKind::Doc);
        assert_eq!(source.size, 5);
        assert_eq!(source.blake3, blake3_hex(b"hello"));
        assert_eq!(source.as_disk_source().file_path, "README.md");
    }

    #[test]
    fn module_sources_are_deterministic_and_hash_stable() {
        let root = PathBuf::from("/repo");
        let symbols = vec![
            symbol("/repo/src/api/handler.rs", "handle", "function"),
            symbol("/repo/src/api/router.rs", "route", "function"),
            symbol("/repo/src/db/pool.rs", "connect", "function"),
        ];
        let first = build_module_sources(&symbols, &root);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].path, "code://src/api");
        assert_eq!(first[1].path, "code://src/db");
        assert_eq!(first[0].kind, SourceKind::CodeModule);

        let shuffled = vec![symbols[2].clone(), symbols[0].clone(), symbols[1].clone()];
        let second = build_module_sources(&shuffled, &root);
        assert_eq!(first, second, "symbol order must not change the hash");
    }

    #[test]
    fn module_sources_skip_ignored_directories_and_loose_files() {
        let root = PathBuf::from("/repo");
        let symbols = vec![
            symbol("/repo/target/debug/build.rs", "gen", "function"),
            symbol("/repo/build.rs", "main", "function"),
            symbol("/repo/src/lib.rs", "run", "function"),
        ];
        let sources = build_module_sources(&symbols, &root);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].path, "code://src");
    }

    #[test]
    fn collecting_files_walks_docs_and_configs_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("README.md"), "# Hi").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/arch.md"), "# Arch").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/notes.md"), "generated").unwrap();

        let sources = collect_file_sources(root);
        let paths: Vec<&str> = sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["Cargo.toml", "README.md", "docs/arch.md"]);
    }

    #[test]
    fn oversized_files_are_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(
            root.join("big.md"),
            "x".repeat(MAX_SOURCE_BYTES as usize + 1),
        )
        .unwrap();
        std::fs::write(root.join("small.md"), "ok").unwrap();

        let sources = collect_file_sources(root);
        let paths: Vec<&str> = sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["small.md"]);
    }

    /// cas-c736 encoding fixtures.
    fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
        let mut bytes = if little_endian {
            vec![0xFF, 0xFE]
        } else {
            vec![0xFE, 0xFF]
        };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        bytes
    }

    #[test]
    fn utf16_and_bom_marked_docs_are_loaded_rather_than_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("le.md"), utf16("# Little endian\n", true)).unwrap();
        std::fs::write(root.join("be.md"), utf16("# Big endian\n", false)).unwrap();
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"# Bom stripped\n");
        std::fs::write(root.join("bom.md"), bom).unwrap();

        let scan = scan_file_sources(root);
        let paths: Vec<&str> = scan.sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["be.md", "bom.md", "le.md"]);
        assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);

        let by_path = |name: &str| -> String {
            scan.sources
                .iter()
                .find(|s| s.path == name)
                .expect("source present")
                .content
                .clone()
        };
        assert_eq!(by_path("le.md"), "# Little endian\n");
        assert_eq!(by_path("be.md"), "# Big endian\n");
        // The BOM must be gone, not merely tolerated: left on, the distiller
        // reads the first heading as "\u{feff}# Bom stripped".
        assert_eq!(by_path("bom.md"), "# Bom stripped\n");
    }

    #[test]
    fn an_undecodable_doc_is_reported_by_name_instead_of_vanishing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("ok.md"), "# Fine").unwrap();
        // Not UTF-8, no BOM to name another encoding: a latin-1 byte.
        std::fs::write(root.join("broken.md"), [b'#', b' ', 0xFF, b'\n']).unwrap();

        let scan = scan_file_sources(root);
        let paths: Vec<&str> = scan.sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, vec!["ok.md"]);
        assert_eq!(scan.skipped.len(), 1);
        assert_eq!(scan.skipped[0].path, "broken.md");
        assert!(
            scan.skipped[0].reason.contains("not valid UTF-8"),
            "the skip must carry a reason, got {:?}",
            scan.skipped[0].reason
        );
        assert_eq!(
            scan.skip_notes(),
            vec![
                "source skipped: broken.md (not valid UTF-8 and no BOM identifies its encoding)"
                    .to_string()
            ]
        );
    }
}
