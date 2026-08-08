//! Commit ↔ symbol mapping (EPIC cas-6212 / cas-0562, spec §4.1 + §11 M3).
//!
//! For each indexed commit, intersect the **line ranges it changed** with the
//! **line ranges of the symbols** in the files it touched, and record the
//! overlap in `history_commit_symbols`. That is what turns "this commit touched
//! `daemon.rs`" into "this commit touched `should_run_code_index`".
//!
//! # The honesty rule this module exists for (spec §10.2)
//!
//! The symbol index (M2) catches up in the background, so at any moment it may
//! simply not know a file yet. Writing zero symbol rows in that case and
//! stopping would be indistinguishable from "this commit touched no symbols" —
//! a silent empty success, which is the exact dishonesty this epic exists to
//! remove. So every commit is stamped with *why* it has the symbol rows it has:
//! [`SymbolMapping::Absent`] when the index has nothing for the files, `None_`
//! when it has them and nothing overlapped, `Partial` when coverage is mixed.
//! `absent` and `partial` are retried on later passes; they describe the index's
//! coverage at a moment in time, not a property of the commit.
//!
//! # Two identity conventions, bridged here
//!
//! M1 and M2 were built in parallel lanes and chose different keys, both
//! internally consistent, neither aware of the other:
//!
//! | | repository | file path |
//! |---|---|---|
//! | history tables (M1) | canonicalized repo **root path** | repo-**relative** |
//! | code tables (M2) | repo **directory name** | **absolute** |
//!
//! Rewriting either side was rejected: changing M2's identity would orphan the
//! symbols it has already indexed, and changing M1's would invalidate its
//! watermark rows. The bridge is deterministic and lives in exactly one place —
//! [`CommitMapper::symbol_key`] — so there is one place to fix if either side
//! ever converges.
//!
//! # Known limit, stated rather than implied
//!
//! `code_symbols` holds **one** revision's line numbers: whatever `cas index
//! code` last parsed. Intersecting a historical commit's line ranges against
//! them is exact for the most recent commit to touch a file and degrades as
//! edits accumulate above a symbol. Per spec §6.2 this feeds a ranking *boost*,
//! not a fact, so approximation is acceptable — but it is approximation, and
//! callers should not read `history_commit_symbols` as ground truth about what
//! a five-year-old commit contained.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use cas_store::{
    HistoryCommitSymbol, HistoryStore, SOURCE_GIT, SqliteHistoryStore, SymbolMapping, SymbolRange,
};

use super::{repository_id, CHUNK_SIZE};

/// Record separator injected into `--format` so the patch text of one commit
/// can be told from the next. `\x01` cannot appear in a SHA and is not produced
/// by git's own patch framing.
const REC: char = '\u{1}';

/// Default ceiling on commits mapped per pass. Mapping is bounded work per
/// commit (one diff read plus indexed lookups), but a first run over a large
/// backfill should not monopolise a daemon tick.
pub const DEFAULT_MAP_LIMIT: usize = 2_000;

/// A half-open-free, 1-based inclusive line span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineRange {
    pub start: i64,
    pub end: i64,
}

impl LineRange {
    fn overlaps(&self, line_start: i64, line_end: i64) -> bool {
        self.start <= line_end && line_start <= self.end
    }
}

/// What one mapping pass did.
#[derive(Debug, Clone, Default)]
pub struct SymbolMapOutcome {
    pub commits_considered: usize,
    pub symbol_rows: usize,
    /// `symbol_mapping` value → number of commits stamped with it this pass.
    pub verdicts: HashMap<&'static str, usize>,
}

impl SymbolMapOutcome {
    pub fn count(&self, mapping: SymbolMapping) -> usize {
        self.verdicts.get(mapping.as_str()).copied().unwrap_or(0)
    }

    fn record(&mut self, mapping: SymbolMapping) {
        *self.verdicts.entry(mapping.as_str()).or_default() += 1;
    }
}

/// One changed file of one commit, as the mapper needs to see it.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    /// Repo-relative, post-change path.
    pub path: String,
    /// Line ranges touched in the commit's post-image.
    pub ranges: Vec<LineRange>,
}

// ---------------------------------------------------------------------------
// Eligibility
// ---------------------------------------------------------------------------

/// Is this path one the symbol index would ever hold?
///
/// Deliberately driven by the **same** extension list `cas index code` walks
/// (`CodeConfig.extensions`) rather than by a second copy of the language table.
/// That equivalence is what makes the degradation signal precise: "eligible but
/// missing from the index" then means exactly "M2 has not caught up", and never
/// "this file was never indexable in the first place".
pub fn is_eligible_path(path: &str, extensions: &HashSet<String>) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| extensions.contains(&e.to_lowercase()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Diff parsing
// ---------------------------------------------------------------------------

/// Parse `@@ -a,b +c,d @@` and return the post-image span.
///
/// `d == 0` is a pure deletion: nothing exists at that point in the new file,
/// and git reports the line *before* the removal. It is anchored to that line
/// rather than dropped, because a commit that deletes a function's body has
/// unquestionably touched that function, and dropping the hunk would report the
/// commit as touching nothing.
fn parse_hunk_header(line: &str) -> Option<LineRange> {
    let rest = line.strip_prefix("@@ ")?;
    let plus = rest.split(' ').find(|part| part.starts_with('+'))?;
    let body = plus.strip_prefix('+')?;
    let (start, count) = match body.split_once(',') {
        Some((s, c)) => (s.parse::<i64>().ok()?, c.parse::<i64>().ok()?),
        None => (body.parse::<i64>().ok()?, 1),
    };
    if count <= 0 {
        let anchor = start.max(1);
        return Some(LineRange {
            start: anchor,
            end: anchor,
        });
    }
    Some(LineRange {
        start: start.max(1),
        end: start.max(1) + count - 1,
    })
}

/// Undo git's C-style quoting of a `+++ b/…` path.
///
/// git quotes paths containing control characters, quotes or backslashes even
/// with `core.quotePath=false`, so the unquoting cannot be skipped by
/// configuration alone. Anything that is not quoted is returned verbatim.
fn unquote_diff_path(raw: &str) -> String {
    let Some(inner) = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return raw.to_string();
    };
    // Decoded into BYTES, not chars: git emits non-ASCII as a run of octal
    // escapes over the UTF-8 encoding (é is `\303\251`), so pushing each escape
    // as its own `char` would produce mojibake rather than the original name.
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('t') => out.push(b'\t'),
            Some('r') => out.push(b'\r'),
            Some('"') => out.push(b'"'),
            Some('\\') => out.push(b'\\'),
            Some(d) if d.is_digit(8) => {
                let mut digits = String::from(d);
                while digits.len() < 3 {
                    match chars.peek() {
                        Some(next) if next.is_digit(8) => {
                            digits.push(*next);
                            chars.next();
                        }
                        _ => break,
                    }
                }
                match u8::from_str_radix(&digits, 8) {
                    Ok(byte) => out.push(byte),
                    // A malformed escape degrades to its literal text rather
                    // than losing the path entirely.
                    Err(_) => {
                        out.push(b'\\');
                        out.extend_from_slice(digits.as_bytes());
                    }
                }
            }
            Some(other) => {
                let mut buf = [0u8; 4];
                out.push(b'\\');
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a `\x01`-framed batch of `-U0` patches into `sha -> path -> ranges`.
pub fn parse_patch_ranges(raw: &str) -> HashMap<String, HashMap<String, Vec<LineRange>>> {
    let mut out: HashMap<String, HashMap<String, Vec<LineRange>>> = HashMap::new();

    for record in raw.split(REC).skip(1) {
        let mut lines = record.lines();
        let Some(sha) = lines.next().map(str::trim) else {
            continue;
        };
        if sha.len() != 40 {
            continue;
        }
        let per_file = out.entry(sha.to_string()).or_default();

        let mut current: Option<String> = None;
        for line in lines {
            if let Some(path) = line.strip_prefix("+++ ") {
                let path = path.trim();
                current = if path == "/dev/null" {
                    None
                } else {
                    // `b/` is git's default destination prefix; unquote first so
                    // the prefix strip operates on the decoded path.
                    let decoded = unquote_diff_path(path);
                    Some(
                        decoded
                            .strip_prefix("b/")
                            .map(str::to_string)
                            .unwrap_or(decoded),
                    )
                };
                continue;
            }
            if line.starts_with("@@ ") {
                if let (Some(path), Some(range)) = (current.as_ref(), parse_hunk_header(line)) {
                    per_file.entry(path.clone()).or_default().push(range);
                }
            }
        }
    }

    out
}

/// Read `-U0` patches for an explicit commit set, SHAs fed on stdin so a large
/// chunk cannot overflow the command line.
fn git_patch_over(repo_root: &Path, shas: &[String]) -> Result<String> {
    use std::io::Write;

    let format = format!("--format={REC}%H");
    let mut child = Command::new("git")
        .args(["-c", "core.quotePath=false"])
        .args([
            "log",
            "--no-walk",
            "--stdin",
            "--no-color",
            "--unified=0",
            "-M",
            // Deleted files have no post-image, so there is nothing to
            // intersect and nothing to learn from their (empty) ranges.
            "--diff-filter=d",
            &format,
        ])
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning git log for -U0 ranges")?;

    {
        let mut stdin = child.stdin.take().context("git log stdin unavailable")?;
        stdin.write_all(shas.join("\n").as_bytes())?;
        stdin.write_all(b"\n")?;
    }

    let out = child.wait_with_output().context("waiting on git log")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git log -U0 failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---------------------------------------------------------------------------
// The mapping decision
// ---------------------------------------------------------------------------

/// Everything the verdict depends on, so the decision itself stays pure and
/// testable without a repo, a database or a daemon.
pub struct CommitMapper<'a> {
    pub repo_root: &'a Path,
    pub extensions: &'a HashSet<String>,
}

impl<'a> CommitMapper<'a> {
    /// The single place the M1↔M2 identity conventions are bridged.
    pub fn symbol_key(&self, relative_path: &str) -> String {
        self.repo_root.join(relative_path).to_string_lossy().to_string()
    }

    /// Decide one commit's symbol rows and verdict.
    ///
    /// `lookup` answers M2's view of a file: `None` = never indexed (the
    /// degradation signal), `Some(ranges)` = indexed, possibly with no symbols.
    pub fn map_commit<F>(
        &self,
        sha: &str,
        changed: &[ChangedFile],
        mut lookup: F,
    ) -> Result<(Vec<HistoryCommitSymbol>, SymbolMapping)>
    where
        F: FnMut(&str) -> Result<Option<Vec<SymbolRange>>>,
    {
        let eligible: Vec<&ChangedFile> = changed
            .iter()
            .filter(|f| is_eligible_path(&f.path, self.extensions))
            .collect();

        if eligible.is_empty() {
            // Docs, config, binaries, or a merge with no first-parent diff.
            // Nothing here is lag; there was never anything to map.
            return Ok((Vec::new(), SymbolMapping::NotApplicable));
        }

        let mut rows: Vec<HistoryCommitSymbol> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut indexed_files = 0usize;

        for file in eligible.iter() {
            let Some(symbols) = lookup(&self.symbol_key(&file.path))? else {
                continue; // M2 has not seen this file.
            };
            indexed_files += 1;

            for symbol in symbols {
                if !file
                    .ranges
                    .iter()
                    .any(|r| r.overlaps(symbol.line_start, symbol.line_end))
                {
                    continue;
                }
                // The PK is (sha, symbol_id); a symbol reached through two
                // hunks of the same commit is one row, not a conflict.
                if !seen_ids.insert(symbol.symbol_id.clone()) {
                    continue;
                }
                rows.push(HistoryCommitSymbol {
                    sha: sha.to_string(),
                    symbol_id: symbol.symbol_id,
                    qualified_name: symbol.qualified_name,
                    // Repo-relative, deliberately: history rows stay valid
                    // across checkouts, unlike M2's absolute paths.
                    file_path: file.path.clone(),
                });
            }
        }

        let mapping = if indexed_files == 0 {
            // The spec §10.2 row: eligible files exist, the index knows none of
            // them. Answer "I cannot tell you yet", never a bare empty result.
            SymbolMapping::Absent
        } else if indexed_files < eligible.len() {
            SymbolMapping::Partial
        } else if rows.is_empty() {
            // Fully covered and genuinely nothing overlapped — a trustworthy
            // zero (an import block, a top-level comment, a licence header).
            SymbolMapping::None_
        } else {
            SymbolMapping::Mapped
        };

        Ok((rows, mapping))
    }
}

// ---------------------------------------------------------------------------
// The pass
// ---------------------------------------------------------------------------

/// Map symbols for commits that still need it (`pending`, `absent`, `partial`).
///
/// Bounded by `limit`. Errors are recorded on the history state row before
/// propagating so a failed pass is visible in `cas history status` rather than
/// only in a log the user never reads.
pub fn run_symbol_mapping_pass(
    cas_root: &Path,
    repo_root: &Path,
    extensions: &[String],
    limit: usize,
) -> Result<SymbolMapOutcome> {
    let store = SqliteHistoryStore::open(cas_root)?;
    let repository = repository_id(repo_root);

    match map_with_store(&store, repo_root, &repository, extensions, limit) {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            let _ = store.record_attempt(&repository, SOURCE_GIT, Some(&e.to_string()));
            Err(e)
        }
    }
}

pub(crate) fn map_with_store(
    store: &SqliteHistoryStore,
    repo_root: &Path,
    repository: &str,
    extensions: &[String],
    limit: usize,
) -> Result<SymbolMapOutcome> {
    let shas = store.commits_awaiting_symbol_mapping(repository, limit)?;
    let mut outcome = SymbolMapOutcome::default();
    if shas.is_empty() {
        return Ok(outcome);
    }

    let extensions: HashSet<String> = extensions.iter().map(|e| e.to_lowercase()).collect();
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let mapper = CommitMapper {
        repo_root,
        extensions: &extensions,
    };

    for chunk in shas.chunks(CHUNK_SIZE) {
        let raw = git_patch_over(repo_root, chunk)?;
        let ranges = parse_patch_ranges(&raw);

        // One lookup cache per chunk: a file touched by fifty commits is read
        // from `code_symbols` once, not fifty times.
        let mut cache: HashMap<String, Option<Vec<SymbolRange>>> = HashMap::new();

        let mut mappings: Vec<(String, SymbolMapping)> = Vec::with_capacity(chunk.len());
        let mut rows: Vec<HistoryCommitSymbol> = Vec::new();

        for sha in chunk {
            let changed: Vec<ChangedFile> = ranges
                .get(sha)
                .map(|per_file| {
                    per_file
                        .iter()
                        .map(|(path, ranges)| ChangedFile {
                            path: path.clone(),
                            ranges: ranges.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let (commit_rows, mapping) = mapper.map_commit(sha, &changed, |key| {
                if let Some(hit) = cache.get(key) {
                    return Ok(hit.clone());
                }
                let value = store.symbol_ranges_for_file(&repo_name, key)?;
                cache.insert(key.to_string(), value.clone());
                Ok(value)
            })?;

            outcome.record(mapping);
            outcome.commits_considered += 1;
            rows.extend(commit_rows);
            mappings.push((sha.clone(), mapping));
        }

        outcome.symbol_rows += store.record_symbol_mapping(&mappings, &rows)?;
    }

    Ok(outcome)
}

/// Resolve the extension list the symbol index actually uses, so eligibility
/// here and indexing there cannot drift apart. Reads the same
/// `CodeConfig.extensions` that `cas index code` walks.
pub fn indexable_extensions(cas_root: &Path) -> Vec<String> {
    crate::config::Config::load(cas_root)
        .unwrap_or_default()
        .code()
        .extensions
        .clone()
}

/// Convenience for the CLI and the daemon: map with the configured extensions.
pub fn map_symbols(cas_root: &Path, repo_root: &Path, limit: usize) -> Result<SymbolMapOutcome> {
    run_symbol_mapping_pass(cas_root, repo_root, &indexable_extensions(cas_root), limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions() -> HashSet<String> {
        ["rs", "ts", "py"].iter().map(|e| e.to_string()).collect()
    }

    fn mapper<'a>(root: &'a Path, ext: &'a HashSet<String>) -> CommitMapper<'a> {
        CommitMapper {
            repo_root: root,
            extensions: ext,
        }
    }

    fn sym(id: &str, name: &str, start: i64, end: i64) -> SymbolRange {
        SymbolRange {
            symbol_id: id.into(),
            qualified_name: name.into(),
            line_start: start,
            line_end: end,
        }
    }

    fn changed(path: &str, ranges: &[(i64, i64)]) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            ranges: ranges
                .iter()
                .map(|(s, e)| LineRange { start: *s, end: *e })
                .collect(),
        }
    }

    // ---- hunk headers ----

    #[test]
    fn hunk_header_parses_a_multi_line_addition() {
        assert_eq!(
            parse_hunk_header("@@ -10,0 +11,3 @@ fn foo()"),
            Some(LineRange {
                start: 11,
                end: 13
            })
        );
    }

    #[test]
    fn hunk_header_without_a_count_is_one_line() {
        assert_eq!(
            parse_hunk_header("@@ -4 +4 @@"),
            Some(LineRange { start: 4, end: 4 })
        );
    }

    /// `+9,0` is a pure deletion. Anchoring it to line 9 rather than dropping it
    /// is what keeps "this commit deleted the body of `foo`" from reporting as
    /// "this commit touched nothing".
    #[test]
    fn pure_deletion_anchors_to_the_preceding_line() {
        assert_eq!(
            parse_hunk_header("@@ -10,3 +9,0 @@"),
            Some(LineRange { start: 9, end: 9 })
        );
    }

    /// A deletion at the very top of a file reports `+0,0`; line 0 does not
    /// exist, and an unclamped 0 would never intersect a 1-based symbol range.
    #[test]
    fn deletion_at_start_of_file_clamps_to_line_one() {
        assert_eq!(
            parse_hunk_header("@@ -1,4 +0,0 @@"),
            Some(LineRange { start: 1, end: 1 })
        );
    }

    #[test]
    fn non_hunk_lines_are_rejected() {
        assert_eq!(parse_hunk_header("+++ b/src/lib.rs"), None);
        assert_eq!(parse_hunk_header("@@ garbage @@"), None);
    }

    // ---- path unquoting ----

    #[test]
    fn unquoted_paths_pass_through() {
        assert_eq!(unquote_diff_path("b/src/lib.rs"), "b/src/lib.rs");
        assert_eq!(unquote_diff_path("b/has space.rs"), "b/has space.rs");
    }

    #[test]
    fn quoted_paths_are_decoded() {
        assert_eq!(unquote_diff_path(r#""b/a\tb.rs""#), "b/a\tb.rs");
        assert_eq!(unquote_diff_path(r#""b/say \"hi\".rs""#), "b/say \"hi\".rs");
        assert_eq!(unquote_diff_path(r#""b/back\\slash.rs""#), "b/back\\slash.rs");
    }

    #[test]
    fn octal_escapes_decode_to_bytes() {
        // git renders a UTF-8 é as \303\251 when quoting; the two escapes are
        // the two bytes of ONE character and must recombine into it.
        assert_eq!(unquote_diff_path(r#""b/caf\303\251.rs""#), "b/café.rs");
    }

    // ---- patch batch parsing ----

    #[test]
    fn patch_batch_splits_per_commit_and_per_file() {
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        let raw = format!(
            "\u{1}{sha_a}\n\
             diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,0 +2,2 @@\n\
             +one\n\
             +two\n\
             diff --git a/README.md b/README.md\n\
             --- a/README.md\n\
             +++ b/README.md\n\
             @@ -5 +5 @@\n\
             \u{1}{sha_b}\n\
             diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -20,0 +21,1 @@\n"
        );

        let parsed = parse_patch_ranges(&raw);
        assert_eq!(parsed.len(), 2);
        let a = &parsed[&sha_a];
        assert_eq!(a["src/lib.rs"], vec![LineRange { start: 2, end: 3 }]);
        assert_eq!(a["README.md"], vec![LineRange { start: 5, end: 5 }]);
        assert_eq!(
            parsed[&sha_b]["src/lib.rs"],
            vec![LineRange {
                start: 21,
                end: 21
            }]
        );
    }

    /// A newly added file diffs against `/dev/null` on the `---` side but has a
    /// real `+++` path; a *deleted* file is the reverse, and its hunks must not
    /// be attributed to whatever file happened to precede it.
    #[test]
    fn dev_null_destination_drops_its_hunks() {
        let sha = "c".repeat(40);
        let raw = format!(
            "\u{1}{sha}\n\
             --- a/src/keep.rs\n\
             +++ b/src/keep.rs\n\
             @@ -1 +1 @@\n\
             --- a/src/gone.rs\n\
             +++ /dev/null\n\
             @@ -1,50 +0,0 @@\n"
        );
        let parsed = parse_patch_ranges(&raw);
        let files = &parsed[&sha];
        assert_eq!(files.len(), 1, "deleted file must not contribute ranges");
        assert!(files.contains_key("src/keep.rs"));
    }

    // ---- the mapping decision ----

    /// AC1: a commit touching one function maps to exactly that symbol.
    #[test]
    fn a_commit_touching_one_function_maps_to_exactly_that_symbol() {
        let ext = extensions();
        let root = Path::new("/repo");
        let m = mapper(root, &ext);

        let (rows, mapping) = m
            .map_commit(
                "sha1",
                &[changed("src/lib.rs", &[(12, 14)])],
                |_| {
                    Ok(Some(vec![
                        sym("id-alpha", "lib::alpha", 1, 10),
                        sym("id-beta", "lib::beta", 11, 20),
                        sym("id-gamma", "lib::gamma", 21, 30),
                    ]))
                },
            )
            .unwrap();

        assert_eq!(mapping, SymbolMapping::Mapped);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].qualified_name, "lib::beta");
        assert_eq!(rows[0].symbol_id, "id-beta");
        assert_eq!(
            rows[0].file_path, "src/lib.rs",
            "history rows keep the repo-relative path, not M2's absolute one"
        );
    }

    /// AC2: an unindexed file yields `absent`, never an empty success.
    #[test]
    fn an_unindexed_file_yields_absent_not_an_empty_success() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (rows, mapping) = mapper(root, &ext)
            .map_commit("sha1", &[changed("src/lib.rs", &[(1, 5)])], |_| Ok(None))
            .unwrap();

        assert!(rows.is_empty());
        assert_eq!(
            mapping,
            SymbolMapping::Absent,
            "no symbol index for the file must read as 'cannot tell', not as 'nothing'"
        );
        assert!(
            mapping.is_retryable(),
            "absent describes index coverage at a moment, so it must be retried"
        );
    }

    /// The distinction the spec insists on: `none` is a real zero and must not
    /// be reachable when the index simply has no data.
    #[test]
    fn an_indexed_file_with_no_overlap_yields_none_not_absent() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (rows, mapping) = mapper(root, &ext)
            .map_commit("sha1", &[changed("src/lib.rs", &[(1, 2)])], |_| {
                Ok(Some(vec![sym("id-alpha", "lib::alpha", 40, 60)]))
            })
            .unwrap();

        assert!(rows.is_empty());
        assert_eq!(mapping, SymbolMapping::None_);
        assert!(
            !mapping.is_retryable(),
            "a fully-covered zero is settled and must not be re-mapped forever"
        );
    }

    /// A file the index parsed but which produced no symbols is *covered*, not
    /// lagging. Keying absence on `code_symbols` (as spec §4.1 words it) would
    /// mark such a file as index lag that never clears.
    #[test]
    fn an_indexed_but_symbol_free_file_is_covered_not_absent() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (_, mapping) = mapper(root, &ext)
            .map_commit("sha1", &[changed("src/empty.rs", &[(1, 1)])], |_| {
                Ok(Some(Vec::new()))
            })
            .unwrap();
        assert_eq!(mapping, SymbolMapping::None_);
    }

    #[test]
    fn mixed_coverage_yields_partial_and_still_records_what_it_can() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (rows, mapping) = mapper(root, &ext)
            .map_commit(
                "sha1",
                &[
                    changed("src/covered.rs", &[(5, 5)]),
                    changed("src/uncovered.rs", &[(5, 5)]),
                ],
                |key| {
                    Ok(if key.ends_with("/covered.rs") {
                        Some(vec![sym("id-a", "covered::a", 1, 10)])
                    } else {
                        None
                    })
                },
            )
            .unwrap();

        assert_eq!(mapping, SymbolMapping::Partial);
        assert_eq!(rows.len(), 1, "the covered half must still be recorded");
        assert!(mapping.is_retryable());
    }

    /// A docs-only commit is not index lag; reporting it as `absent` would be a
    /// permanent false alarm on a bucket operators are meant to act on.
    #[test]
    fn a_commit_with_no_indexable_file_is_not_applicable_not_absent() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (rows, mapping) = mapper(root, &ext)
            .map_commit(
                "sha1",
                &[changed("README.md", &[(1, 3)]), changed("logo.png", &[])],
                |_| panic!("ineligible files must never reach the symbol index"),
            )
            .unwrap();

        assert!(rows.is_empty());
        assert_eq!(mapping, SymbolMapping::NotApplicable);
        assert!(!mapping.is_retryable());
    }

    /// A merge commit (or any commit whose diff is empty) has no changed files
    /// at all and must not be mistaken for missing index data.
    #[test]
    fn a_commit_with_no_diff_is_not_applicable() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (_, mapping) = mapper(root, &ext)
            .map_commit("merge", &[], |_| panic!("nothing to look up"))
            .unwrap();
        assert_eq!(mapping, SymbolMapping::NotApplicable);
    }

    /// The table's PK is `(sha, symbol_id)`. A symbol reached through several
    /// hunks of one commit is one row; emitting it twice would make the
    /// transactional insert fight its own primary key.
    #[test]
    fn a_symbol_hit_by_several_hunks_is_recorded_once() {
        let ext = extensions();
        let root = Path::new("/repo");
        let (rows, _) = mapper(root, &ext)
            .map_commit(
                "sha1",
                &[changed("src/lib.rs", &[(3, 3), (7, 7), (9, 9)])],
                |_| Ok(Some(vec![sym("id-a", "lib::a", 1, 20)])),
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    /// Boundary lines are inside the symbol: a change on a function's very
    /// first or last line touches that function.
    #[test]
    fn symbol_boundaries_are_inclusive() {
        let ext = extensions();
        let root = Path::new("/repo");
        for line in [10, 20] {
            let (rows, _) = mapper(root, &ext)
                .map_commit("sha1", &[changed("src/lib.rs", &[(line, line)])], |_| {
                    Ok(Some(vec![sym("id-a", "lib::a", 10, 20)]))
                })
                .unwrap();
            assert_eq!(rows.len(), 1, "line {line} must count as inside 10..=20");
        }

        let (rows, _) = mapper(root, &ext)
            .map_commit("sha1", &[changed("src/lib.rs", &[(21, 21)])], |_| {
                Ok(Some(vec![sym("id-a", "lib::a", 10, 20)]))
            })
            .unwrap();
        assert!(rows.is_empty(), "line 21 is outside 10..=20");
    }

    #[test]
    fn eligibility_follows_the_configured_extension_list() {
        let ext = extensions();
        assert!(is_eligible_path("src/lib.rs", &ext));
        assert!(is_eligible_path("src/LIB.RS", &ext));
        assert!(!is_eligible_path("README.md", &ext));
        assert!(!is_eligible_path("Makefile", &ext));
    }

    /// The one place M1's and M2's identity conventions meet.
    #[test]
    fn symbol_key_bridges_relative_history_paths_to_absolute_index_paths() {
        let ext = extensions();
        let root = Path::new("/home/dev/cas-src");
        assert_eq!(
            mapper(root, &ext).symbol_key("cas-cli/src/lib.rs"),
            "/home/dev/cas-src/cas-cli/src/lib.rs"
        );
    }
}
