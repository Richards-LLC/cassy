//! The distillation pass (EPIC cas-7d31 / cas-c9be).
//!
//! One pass is: classify sources against the ledger → distill only what moved →
//! merge each distilled page into its canonical path → commit everything in one
//! transaction → repair provenance and wikilinks left behind by deletions.
//!
//! Cost discipline is the point. An unchanged repo performs **zero** LLM calls
//! because the ledger short-circuit happens before any prompt is built, and a
//! page that already states the incoming material merges for free (tier a).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use chrono::Utc;

use cas_store::{
    IngestBatch, KnowledgePage, KnowledgeStore, PageWrite, SourceOutcome, SourceStatus,
    canonical_rel_path, classify_sources, slugify,
};

use super::chunk::{ChunkOptions, chunk_markdown};
use super::llm::LlmRunner;
use super::merge::{
    self, DEFAULT_SMALL_PAGE_CHARS, Fragment, Frontmatter, MergeTier, StripOutcome,
};
use super::prompt;
use super::sources::{LoadedSource, disk_sources};

/// Knobs for one distillation pass.
#[derive(Debug, Clone)]
pub struct DistillConfig {
    /// Chunking limits for source excerpts.
    pub chunk: ChunkOptions,
    /// Pages at or below this size are rewritten whole instead of appended to.
    pub small_page_chars: usize,
    /// Upper bound on sources distilled in one pass (cost guard). The rest are
    /// deferred to the next pass; they are not marked ingested.
    pub max_sources_per_pass: usize,
    /// Upper bound on chunks distilled per source (cost guard).
    pub max_chunks_per_source: usize,
    /// Plan only: classify against the ledger and report, then stop. Nothing is
    /// prompted, nothing is committed, and the ledger is left untouched — a dry
    /// run must never look like a failed pass to the next real one.
    pub dry_run: bool,
    /// Ledger path prefixes this pass must NOT tombstone even when they are
    /// absent from `sources`.
    ///
    /// This is the escape hatch for a caller that knows its source set is
    /// partial — e.g. the code index was too large to load in full, so the
    /// `code://` sources it produced do not cover every module. Without it,
    /// "I could not enumerate these" is indistinguishable from "these were
    /// deleted", and the pass would cascade-delete live pages.
    pub protected_prefixes: Vec<String>,
}

impl Default for DistillConfig {
    fn default() -> Self {
        Self {
            chunk: ChunkOptions::default(),
            small_page_chars: DEFAULT_SMALL_PAGE_CHARS,
            max_sources_per_pass: 25,
            max_chunks_per_source: 12,
            dry_run: false,
            protected_prefixes: Vec::new(),
        }
    }
}

/// What a pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistillReport {
    pub sources_scanned: usize,
    /// Sources the ledger says need (re-)distillation this pass.
    pub sources_pending: usize,
    /// Sources this pass tried to distill (successes plus failures).
    pub sources_attempted: usize,
    /// Sources distilled successfully and recorded as `ingested`. Disjoint from
    /// [`Self::sources_failed`].
    pub sources_distilled: usize,
    pub sources_skipped: usize,
    pub sources_failed: usize,
    pub sources_deferred: usize,
    pub sources_tombstoned: usize,
    pub pages_written: usize,
    pub pages_locked_skipped: usize,
    pub pages_cascade_deleted: usize,
    pub pages_provenance_rewritten: usize,
    pub pages_flagged_for_redistill: usize,
    pub wikilinks_rewritten: usize,
    pub llm_calls: usize,
    pub tier_union_only: usize,
    pub tier_full_rewrite: usize,
    pub tier_append_delta: usize,
    /// Something went wrong. Non-empty means the pass did not fully succeed.
    pub errors: Vec<String>,
    /// Informational messages (dry-run plans, deferrals). Never a failure
    /// signal — consumers gate health on [`Self::errors`] alone.
    pub notes: Vec<String>,
}

impl DistillReport {
    /// Did this pass change anything durable?
    pub fn is_noop(&self) -> bool {
        self.pages_written == 0
            && self.sources_tombstoned == 0
            && self.pages_cascade_deleted == 0
            && self.pages_provenance_rewritten == 0
    }
}

/// Run one distillation pass over `sources`.
///
/// **Precondition: `sources` must be the COMPLETE source set for the project.**
/// Classification is a set difference against the ledger, so any previously
/// ingested path missing from this slice is treated as deleted from disk — it is
/// tombstoned and its sole-source pages are cascade-deleted. A caller that
/// narrows the set (e.g. omits the `code://` module sources) does not "skip"
/// them, it deletes them.
pub fn run_distillation(
    store: &dyn KnowledgeStore,
    runner: &dyn LlmRunner,
    sources: &[LoadedSource],
    config: &DistillConfig,
) -> Result<DistillReport> {
    let calls_before = runner.calls();
    let mut report = DistillReport {
        sources_scanned: sources.len(),
        ..Default::default()
    };

    let ledger = store.list_sources()?;
    let mut classification = classify_sources(&disk_sources(sources), &ledger);

    // Honour the caller's declaration that part of the source set is unknown
    // rather than gone (see `protected_prefixes`).
    if !config.protected_prefixes.is_empty() {
        let before = classification.deleted.len();
        classification.deleted.retain(|path| {
            !config
                .protected_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix.as_str()))
        });
        let protected = before - classification.deleted.len();
        if protected > 0 {
            report.notes.push(format!(
                "{protected} ledger source(s) left untouched: the caller declared this source set partial"
            ));
        }
    }

    report.sources_skipped = classification.skipped.len();
    report.sources_pending = classification.to_ingest.len();

    // Nothing moved and nothing died: return before a single prompt is built.
    // This is the zero-cost path for an unchanged repo.
    if classification.to_ingest.is_empty() && classification.deleted.is_empty() {
        return Ok(report);
    }

    // A dry run stops here: it has classified everything (which is the useful
    // answer) without prompting, committing, or writing a ledger row.
    if config.dry_run {
        report.notes.extend(
            classification
                .deleted
                .iter()
                .map(|path| format!("dry run: {path} is gone from disk and would be tombstoned")),
        );
        return Ok(report);
    }

    let by_path: HashMap<&str, &LoadedSource> = sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect();

    // Pages that cite a dying source, captured BEFORE the commit strips the
    // provenance out from under us.
    let mut cascade_watch: Vec<(String, Vec<KnowledgePage>)> = Vec::new();
    for path in &classification.deleted {
        let pages = store.pages_for_source(path).unwrap_or_default();
        if !pages.is_empty() {
            cascade_watch.push((path.clone(), pages));
        }
    }

    // ── Distill ─────────────────────────────────────────────────────────
    let mut pending = classification.to_ingest;
    if pending.len() > config.max_sources_per_pass {
        report.sources_deferred = pending.len() - config.max_sources_per_pass;
        pending.truncate(config.max_sources_per_pass);
    }

    let mut writes: BTreeMap<String, PageWrite> = BTreeMap::new();
    let mut outcomes: Vec<SourceOutcome> = Vec::new();

    for candidate in &pending {
        let path = candidate.source.file_path.as_str();
        let Some(loaded) = by_path.get(path).copied() else {
            continue;
        };

        let (pages, mut failure) = distill_source(runner, loaded, config, &mut report);
        report.sources_attempted += 1;

        for page in pages {
            if let Err(error) = stage_page(
                store,
                runner,
                &mut writes,
                loaded,
                page,
                config,
                &mut report,
            ) {
                // A page that could not be staged means this source is NOT
                // fully ingested. Folding the error into `failure` keeps the
                // ledger honest: marking it Ingested here would make
                // `classify_sources` skip it forever at this content hash, so
                // the knowledge would be lost until the file's bytes changed.
                report.errors.push(format!("{path}: {error}"));
                failure.get_or_insert(format!("stage page: {error}"));
            }
        }

        let status = if failure.is_some() {
            report.sources_failed += 1;
            SourceStatus::Failed
        } else {
            report.sources_distilled += 1;
            SourceStatus::Ingested
        };
        outcomes.push(SourceOutcome {
            file_path: loaded.path.clone(),
            blake3: loaded.blake3.clone(),
            size: loaded.size,
            status,
            ingest_error: failure,
        });
    }

    // ── Commit ──────────────────────────────────────────────────────────
    let batch = IngestBatch {
        pages: writes.into_values().collect(),
        sources: outcomes,
        tombstones: classification.deleted.clone(),
    };
    let commit = store.commit_ingest(&batch)?;
    report.pages_written = commit.pages_written;
    report.pages_locked_skipped += commit.locked_skipped_rel_paths.len();
    report.sources_tombstoned = commit.sources_tombstoned;
    report.pages_cascade_deleted = commit.cascade_deleted_page_ids.len();

    // ── Post-commit repair ──────────────────────────────────────────────
    let deleted_ids: BTreeSet<&String> = commit.cascade_deleted_page_ids.iter().collect();
    repair_after_deletions(store, &cascade_watch, &deleted_ids, &mut report)?;

    report.llm_calls = runner.calls().saturating_sub(calls_before);
    Ok(report)
}

/// Two-stage distillation of one source, degrading to single-stage when the
/// extraction plan comes back empty or unparseable.
fn distill_source(
    runner: &dyn LlmRunner,
    source: &LoadedSource,
    config: &DistillConfig,
    report: &mut DistillReport,
) -> (Vec<prompt::DistilledPage>, Option<String>) {
    let hint = source.kind.page_type_hint();
    let mut pages = Vec::new();
    let mut failure: Option<String> = None;

    let chunks = chunk_markdown(&source.content, &config.chunk);
    for chunk in chunks.iter().take(config.max_chunks_per_source) {
        // Stage A — extraction plan.
        let plan = match runner.complete(&prompt::stage_a_prompt(
            &source.path,
            &chunk.heading,
            &chunk.text,
        )) {
            Ok(response) => prompt::parse_plan(&response),
            Err(error) => {
                let message = format!("stage A: {error}");
                report.errors.push(format!("{}: {message}", source.path));
                failure.get_or_insert(message);
                prompt::ExtractionPlan::default()
            }
        };

        // Stage B — page generation, or the single-stage degrade path.
        let stage_b_prompt_text = if plan.is_empty() {
            prompt::single_stage_prompt(&source.path, hint, &chunk.text)
        } else {
            let plan_json = serde_json::to_string(&plan).unwrap_or_default();
            prompt::stage_b_prompt(&source.path, hint, &plan_json, &chunk.text)
        };

        match runner.complete(&stage_b_prompt_text) {
            Ok(response) => pages.extend(prompt::parse_pages(&response)),
            Err(error) => {
                let message = format!("stage B: {error}");
                report.errors.push(format!("{}: {message}", source.path));
                failure.get_or_insert(message);
            }
        }
    }

    (pages, failure)
}

/// The single definition of "the user owns this page".
///
/// Locking has two gestures — the `locked` column (via `set_locked` or the MCP
/// surface) and `locked: true` in the body's frontmatter (hand-editing the
/// markdown). Both must be honoured everywhere a page is rewritten, so every
/// site asks this function rather than re-deriving the rule.
fn is_locked(page: &KnowledgePage, body: &str) -> bool {
    page.locked || merge::is_locked_body(body)
}

/// Are two page titles plainly the same subject?
///
/// Used only to decide whether a drifted title should merge into an existing
/// page. Deliberately conservative: one slug containing the other (e.g.
/// `build-system` vs `the-build-system`) counts, unrelated titles do not, so
/// the failure mode is a duplicate page rather than two subjects fused into one.
fn titles_are_the_same_subject(existing: &str, incoming: &str) -> bool {
    let existing = slugify(existing);
    let incoming = slugify(incoming);
    if existing.is_empty() || incoming.is_empty() {
        return false;
    }
    existing == incoming || existing.contains(&incoming) || incoming.contains(&existing)
}

/// Merge one distilled page into the batch at its canonical path.
#[allow(clippy::too_many_arguments)]
fn stage_page(
    store: &dyn KnowledgeStore,
    runner: &dyn LlmRunner,
    writes: &mut BTreeMap<String, PageWrite>,
    source: &LoadedSource,
    distilled: prompt::DistilledPage,
    config: &DistillConfig,
    report: &mut DistillReport,
) -> Result<()> {
    let page_type = if distilled.page_type.is_empty() {
        source.kind.page_type_hint().to_string()
    } else {
        distilled.page_type.clone()
    };

    // The model's opinion about *where* the page lives is never used: the path
    // is derived from type + title so a re-distillation merges instead of
    // forking a near-duplicate.
    let mut rel_path = canonical_rel_path(&page_type, &distilled.title);

    // Title drift ("Build System" → "The Build System") would fork a duplicate page, so
    // when this source already owns exactly one page of this type AND the titles are the
    // same subject, merge into it by provenance. Both conditions are load-bearing: without
    // the similarity check a source's legitimate SECOND page of that type gets swallowed by
    // the first; without the `writes` check two pages staged in one pass collide on it.
    if !writes.contains_key(&rel_path) && store.get_page_by_rel_path(&rel_path)?.is_none() {
        let siblings: Vec<KnowledgePage> = store
            .pages_for_source(&source.path)
            .unwrap_or_default()
            .into_iter()
            .filter(|page| page.page_type == page_type && !page.locked)
            .collect();
        if siblings.len() == 1
            && !writes.contains_key(&siblings[0].rel_path)
            && titles_are_the_same_subject(&siblings[0].title, &distilled.title)
        {
            rel_path = siblings[0].rel_path.clone();
        }
    }

    // Existing state: this pass's accumulator first, then the store.
    let (mut page, existing_body) = match writes.get(&rel_path) {
        Some(staged) => (staged.page.clone(), staged.body.clone()),
        None => match store.get_page_by_rel_path(&rel_path)? {
            Some(existing) => {
                // Only a genuinely missing file may default to empty. Any other
                // read failure (permissions, EIO, a concurrent rename) must
                // propagate: treating it as "no body" would rebuild the page
                // from scratch and publish that over the real one.
                let body = match store.read_body(&existing.rel_path) {
                    Ok(body) => body,
                    Err(cas_store::StoreError::NotFound(_)) => String::new(),
                    Err(error) => {
                        return Err(anyhow::anyhow!(
                            "cannot read existing body {}: {error}",
                            existing.rel_path
                        ));
                    }
                };
                (existing, body)
            }
            None => {
                let mut fresh =
                    KnowledgePage::new(store.generate_id()?, &page_type, &distilled.title);
                fresh.rel_path = rel_path.clone();
                (fresh, String::new())
            }
        },
    };

    // Locked pages are the user's. Neither the row nor the file is touched, and
    // the skip is reported rather than swallowed.
    if is_locked(&page, &existing_body) {
        report.pages_locked_skipped += 1;
        return Ok(());
    }

    let sources = merge::union_sources(&page.sources, &[source.path.clone()]);
    let mut rewrite_snippet: Option<String> = None;

    let fragments: Vec<Fragment> = if existing_body.trim().is_empty() {
        vec![Fragment::new(
            vec![source.path.clone()],
            // Marker-shaped lines in model prose are defanged before they can
            // become real provenance on the next read (see merge::escape_markers).
            merge::escape_markers(distilled.body.trim()),
        )]
    } else {
        match merge::choose_tier(&existing_body, &distilled.body, config.small_page_chars) {
            MergeTier::UnionSourcesOnly => {
                report.tier_union_only += 1;
                // Body byte-identical; only provenance widens. Zero LLM cost.
                // The new source joins the trailing distilled fragment because it
                // independently attests that span's claim, so on delete the span
                // survives losing either witness and is flagged for re-distillation
                // rather than assumed still accurate.
                let mut existing = merge::split_fragments(&existing_body);
                if let Some(last) = existing.last_mut() {
                    if !last.sources.contains(&source.path) && !last.sources.is_empty() {
                        last.sources.push(source.path.clone());
                        last.sources.sort();
                    }
                }
                existing
            }
            MergeTier::FullRewrite => {
                report.tier_full_rewrite += 1;
                let title = if page.title.trim().is_empty() {
                    distilled.title.clone()
                } else {
                    page.title.clone()
                };
                let (fragments, snippet) =
                    rewrite_small_page(runner, &title, &existing_body, &distilled, source, report);
                rewrite_snippet = snippet;
                fragments
            }
            MergeTier::AppendDelta => {
                report.tier_append_delta += 1;
                merge::append_delta(&existing_body, &distilled.body, &source.path, Utc::now())
            }
        }
    };

    page.page_type = page_type;
    if page.title.trim().is_empty() {
        page.title = distilled.title.clone();
    }
    page.sources = sources.clone();
    page.updated_at = Utc::now();
    page.pending_embedding = true;
    if let Some(snippet) = rewrite_snippet {
        page.snippet = snippet;
    } else if !distilled.snippet.is_empty() {
        page.snippet = distilled.snippet.clone();
    }

    let meta = Frontmatter {
        title: page.title.clone(),
        page_type: page.page_type.clone(),
        sources,
        locked: false,
        updated: Some(page.updated_at.to_rfc3339()),
        // Keys Cassy does not own survive the round trip.
        passthrough: merge::parse_frontmatter(&existing_body).passthrough,
    };
    let body = merge::compose_body(&meta, &fragments);

    writes.insert(rel_path, PageWrite { page, body });
    Ok(())
}

/// Tier (b): one LLM call restates the distilled prose as a single voice.
///
/// The hand-written preamble (the leading fragment with no provenance) is held
/// out of the rewrite entirely and re-prepended afterwards — it is neither shown
/// to the model, nor paraphrased by it, nor re-tagged with distilled provenance
/// that a later cascade delete could take it away with.
///
/// A failed or unusable rewrite degrades to tier (c), so new material is never
/// dropped and existing prose is never lost.
fn rewrite_small_page(
    runner: &dyn LlmRunner,
    title: &str,
    existing_body: &str,
    distilled: &prompt::DistilledPage,
    source: &LoadedSource,
    report: &mut DistillReport,
) -> (Vec<Fragment>, Option<String>) {
    let existing_fragments = merge::split_fragments(existing_body);
    let (preamble, distilled_fragments): (Vec<Fragment>, Vec<Fragment>) = existing_fragments
        .into_iter()
        .partition(|fragment| fragment.sources.is_empty());

    let existing_text = distilled_fragments
        .iter()
        .map(|fragment| fragment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let response = runner.complete(&prompt::rewrite_prompt(
        title,
        &existing_text,
        &distilled.body,
    ));

    let rewritten = match response {
        Ok(text) => prompt::parse_rewrite(&text),
        Err(error) => {
            report
                .errors
                .push(format!("{}: merge rewrite: {error}", source.path));
            None
        }
    };

    let Some(rewritten) = rewritten else {
        // Degrade to tier (c): append, existing prose untouched.
        report.tier_full_rewrite = report.tier_full_rewrite.saturating_sub(1);
        report.tier_append_delta += 1;
        return (
            merge::append_delta(existing_body, &distilled.body, &source.path, Utc::now()),
            None,
        );
    };

    let mut sources: Vec<String> = distilled_fragments
        .into_iter()
        .flat_map(|fragment| fragment.sources)
        .collect();
    sources.push(source.path.clone());
    sources.sort();
    sources.dedup();

    let mut fragments = preamble;
    fragments.push(Fragment::new(
        sources,
        merge::escape_markers(rewritten.body.trim()),
    ));

    let snippet = (!rewritten.snippet.trim().is_empty()).then(|| rewritten.snippet.clone());
    (fragments, snippet)
}

/// After a commit that tombstoned sources: cut dead provenance out of surviving
/// pages and rewrite wikilinks that now point at nothing.
fn repair_after_deletions(
    store: &dyn KnowledgeStore,
    cascade_watch: &[(String, Vec<KnowledgePage>)],
    deleted_ids: &BTreeSet<&String>,
    report: &mut DistillReport,
) -> Result<()> {
    if cascade_watch.is_empty() {
        return Ok(());
    }

    let mut bodies: BTreeMap<String, (KnowledgePage, String)> = BTreeMap::new();
    let mut redistill: BTreeSet<String> = BTreeSet::new();

    // Titles of the pages this commit actually deleted. Only links to THESE
    // become plain text: the model is told to write `[[Page Title]]` cross
    // references, so a link to a page that has not been distilled yet is a
    // normal forward reference, not a dangling one, and flattening it would
    // destroy it permanently the first time any unrelated source was removed.
    let mut dead_targets: BTreeSet<String> = BTreeSet::new();
    for (_, pages) in cascade_watch {
        for watched in pages {
            if deleted_ids.contains(&watched.id) {
                dead_targets.insert(slugify(&watched.title));
                dead_targets.insert(watched.rel_path.clone());
            }
        }
    }

    for (dead_source, pages) in cascade_watch {
        for watched in pages {
            if deleted_ids.contains(&watched.id) {
                continue; // page went with its last source
            }
            let Ok(current) = store.get_page(&watched.id) else {
                continue;
            };
            let Some(body) = load_body(store, &bodies, &current) else {
                continue; // unreadable: leave it alone rather than clobber it
            };
            if is_locked(&current, &body) {
                continue;
            }
            if let StripOutcome::Rewritten {
                body,
                needs_redistill,
            } = merge::strip_source(&body, dead_source)
            {
                if needs_redistill {
                    redistill.extend(current.sources.iter().cloned());
                    report.pages_flagged_for_redistill += 1;
                }
                report.pages_provenance_rewritten += 1;
                bodies.insert(current.id.clone(), (current, body));
            }
        }
    }

    // Wikilink repair: links into a page this pass deleted become plain text,
    // so a reader never follows a link into a page that no longer exists.
    if !dead_targets.is_empty() {
        for page in store.list_pages()? {
            let Some(body) = load_body(store, &bodies, &page) else {
                continue;
            };
            if is_locked(&page, &body) {
                continue;
            }
            let (rewritten, count) = rewrite_dangling_wikilinks(&body, &dead_targets);
            if count > 0 {
                report.wikilinks_rewritten += count;
                bodies.insert(page.id.clone(), (page, rewritten));
            }
        }
    }

    if bodies.is_empty() {
        return Ok(());
    }

    // Sources whose page prose may still describe a dead sibling are demoted to
    // `uploaded`, which `classify_sources` turns into a Retry next pass.
    let ledger = store.list_sources()?;
    let demotions: Vec<SourceOutcome> = ledger
        .into_iter()
        .filter(|row| redistill.contains(&row.file_path))
        .map(|row| SourceOutcome {
            file_path: row.file_path,
            blake3: row.blake3,
            size: row.size,
            status: SourceStatus::Uploaded,
            ingest_error: None,
        })
        .collect();

    let batch = IngestBatch {
        pages: bodies
            .into_values()
            .map(|(mut page, body)| {
                // The prose changed, so any embedding computed for it is stale.
                page.pending_embedding = true;
                page.updated_at = Utc::now();
                PageWrite { page, body }
            })
            .collect(),
        sources: demotions,
        tombstones: Vec::new(),
    };
    store.commit_ingest(&batch)?;
    Ok(())
}

/// Body of a page during the repair pass: the in-flight edit if there is one,
/// otherwise the file. `None` means unreadable — the caller must then leave the
/// page alone rather than rewrite it from an assumed-empty body.
fn load_body(
    store: &dyn KnowledgeStore,
    edited: &BTreeMap<String, (KnowledgePage, String)>,
    page: &KnowledgePage,
) -> Option<String> {
    if let Some((_, body)) = edited.get(&page.id) {
        return Some(body.clone());
    }
    match store.read_body(&page.rel_path) {
        Ok(body) => Some(body),
        Err(cas_store::StoreError::NotFound(_)) => Some(String::new()),
        Err(_) => None,
    }
}

/// Replace `[[Target]]` / `[[Target|Text]]` with plain text when `Target` names
/// a page that was just deleted. Returns the new body and the rewrite count.
pub fn rewrite_dangling_wikilinks(body: &str, dead_targets: &BTreeSet<String>) -> (String, usize) {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    let mut rewritten = 0usize;

    while let Some(start) = rest.find("[[") {
        let Some(end_offset) = rest[start + 2..].find("]]") else {
            break;
        };
        let inner = &rest[start + 2..start + 2 + end_offset];
        out.push_str(&rest[..start]);

        let (target, display) = match inner.split_once('|') {
            Some((target, display)) => (target.trim(), display.trim()),
            None => (inner.trim(), inner.trim()),
        };

        if dead_targets.contains(&slugify(target)) || dead_targets.contains(target) {
            out.push_str(display);
            rewritten += 1;
        } else {
            out.push_str(&rest[start..start + 4 + end_offset]);
        }
        rest = &rest[start + 4 + end_offset..];
    }

    out.push_str(rest);
    (out, rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dead(entries: &[&str]) -> BTreeSet<String> {
        entries.iter().map(|entry| slugify(entry)).collect()
    }

    #[test]
    fn links_to_live_pages_are_left_alone() {
        let dead = dead(&["Removed Page"]);
        let (body, count) = rewrite_dangling_wikilinks("see [[Build System]] now", &dead);
        assert_eq!(count, 0);
        assert_eq!(body, "see [[Build System]] now");
    }

    #[test]
    fn links_to_a_page_that_was_never_created_survive() {
        // The model is told to write cross references, so a link to a page that
        // has not been distilled YET is a forward reference, not a dangling one.
        // Flattening it would destroy it permanently.
        let dead = dead(&["Removed Page"]);
        let (body, count) = rewrite_dangling_wikilinks("see [[Not Yet Written]]", &dead);
        assert_eq!(count, 0);
        assert_eq!(body, "see [[Not Yet Written]]");
    }

    #[test]
    fn links_to_a_deleted_page_become_plain_text() {
        let dead = dead(&["Removed Page"]);
        let (body, count) =
            rewrite_dangling_wikilinks("see [[Removed Page]] and [[Build System]]", &dead);
        assert_eq!(count, 1);
        assert_eq!(body, "see Removed Page and [[Build System]]");
    }

    #[test]
    fn piped_links_keep_their_display_text() {
        let dead = dead(&["Removed Page"]);
        let (body, count) =
            rewrite_dangling_wikilinks("[[Removed Page|the old thing]] and [[Alive|it]]", &dead);
        assert_eq!(count, 1);
        assert_eq!(body, "the old thing and [[Alive|it]]");
    }

    #[test]
    fn unterminated_wikilinks_do_not_eat_the_body() {
        let dead = dead(&["Removed Page"]);
        let (body, count) = rewrite_dangling_wikilinks("text [[unterminated", &dead);
        assert_eq!(count, 0);
        assert_eq!(body, "text [[unterminated");
    }

    #[test]
    fn every_durable_effect_makes_a_pass_non_noop() {
        assert!(DistillReport::default().is_noop());
        let mutations: Vec<fn(&mut DistillReport)> = vec![
            |report| report.pages_written = 1,
            |report| report.sources_tombstoned = 1,
            |report| report.pages_cascade_deleted = 1,
            |report| report.pages_provenance_rewritten = 1,
        ];
        for mutate in mutations {
            let mut report = DistillReport::default();
            mutate(&mut report);
            assert!(!report.is_noop(), "a durable effect must not read as a no-op");
        }
    }

    #[test]
    fn title_drift_merges_only_for_the_same_subject() {
        assert!(titles_are_the_same_subject("Build System", "The Build System"));
        assert!(titles_are_the_same_subject("Build System", "build system"));
        assert!(!titles_are_the_same_subject("Build System", "Testing"));
        assert!(!titles_are_the_same_subject("Build System", ""));
    }

    #[test]
    fn a_page_is_locked_by_either_gesture() {
        let mut page = KnowledgePage::new("cas-kn001", "guide", "T");
        assert!(!is_locked(&page, "no frontmatter"));
        assert!(is_locked(&page, "---\nlocked: true\n---\n\nmine\n"));
        page.locked = true;
        assert!(is_locked(&page, "no frontmatter"));
    }
}
