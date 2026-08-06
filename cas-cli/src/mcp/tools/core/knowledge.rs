//! MCP handlers for the distilled project wiki (EPIC cas-7d31 / cas-ee3d).
//!
//! These are the *page* operations of the knowledge store — search, read,
//! write, list, status. The belief-typed `opinion_*` handlers that used to
//! occupy this filename now live in [`super::opinion`]; they operate on memory
//! entries and are exposed through the `memory` tool, so nothing here collides
//! with them.
//!
//! Everything reads through `KnowledgeStore`. Bodies never enter SQLite: the
//! index rows are in the DB, the markdown is on disk under `.cas/knowledge/`.

use crate::mcp::tools::core::imports::*;

use cas_mcp::KnowledgeRequest;
use cas_store::{IngestBatch, KnowledgePage, PageWrite, canonical_rel_path};

/// Provenance recorded for a page an agent wrote by hand.
///
/// It is deliberately not a real file path and never enters the source ledger,
/// so the distillation pass can neither tombstone it nor cascade-delete the
/// page it belongs to.
pub(crate) const MANUAL_SOURCE: &str = "manual://mcp";

/// Default page type when a caller does not name one.
const DEFAULT_PAGE_TYPE: &str = "guide";

/// Fallback result cap for `search`/`list`.
const DEFAULT_LIMIT: usize = 20;

/// Longest body echoed back in a `read` response before truncation.
const MAX_BODY_CHARS: usize = 20_000;

/// Derive an index-injectable snippet from a markdown body.
///
/// Takes the first line that is neither blank, a heading, a frontmatter fence,
/// nor a provenance marker — i.e. the first line that actually says something.
fn snippet_from_body(body: &str) -> String {
    let mut in_frontmatter = false;
    for (index, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if line == "---" {
            // A leading `---` opens frontmatter; the next one closes it.
            if index == 0 {
                in_frontmatter = true;
            } else if in_frontmatter {
                in_frontmatter = false;
            }
            continue;
        }
        if in_frontmatter
            || line.is_empty()
            || line.starts_with('#')
            || line.starts_with("<!--")
            || line.starts_with('>')
        {
            continue;
        }
        return truncate_str(line, 280);
    }
    String::new()
}

/// Split a comma-separated provenance list into non-empty trimmed paths.
fn parse_sources(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// One index line describing a page.
fn page_line(page: &KnowledgePage) -> String {
    format!(
        "{} [{}] {} — {} ({})",
        if page.locked { "🔒" } else { "  " },
        page.id,
        page.rel_path,
        page.title,
        page.page_type
    )
}

impl CasCore {
    // ========================================================================
    // Knowledge page tools (distilled project wiki)
    // ========================================================================

    /// Full-text search across distilled page titles, snippets and bodies.
    pub async fn knowledge_search(
        &self,
        Parameters(req): Parameters<KnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let query = req
            .query
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| {
                Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "knowledge search requires a non-empty 'query'",
                )
            })?;

        let store = self.open_knowledge_store()?;
        let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 100);
        let hits = store.search(query, limit).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge search failed: {e}"),
            )
        })?;

        if hits.is_empty() {
            return Ok(Self::success(format!(
                "No distilled pages match '{query}'. Run `cas knowledge build` to distill the repo."
            )));
        }

        let include_body = req.include_body.unwrap_or(false);
        let mut out = format!("Knowledge pages matching '{query}' ({}):\n", hits.len());
        for hit in &hits {
            out.push('\n');
            out.push_str(&page_line(&hit.page));
            if !hit.page.snippet.is_empty() {
                out.push_str(&format!("\n    {}", hit.page.snippet));
            }
            if include_body {
                match store.read_body(&hit.page.rel_path) {
                    Ok(body) => out.push_str(&format!("\n---\n{}\n---", truncate_str(&body, MAX_BODY_CHARS))),
                    Err(e) => out.push_str(&format!("\n    (body unreadable: {e})")),
                }
            }
            out.push('\n');
        }
        out.push_str("\nRead one with: knowledge action=read id=<page-id>");
        Ok(Self::success(out))
    }

    /// Read one page: metadata plus the markdown body from disk.
    pub async fn knowledge_read(
        &self,
        Parameters(req): Parameters<KnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_knowledge_store()?;

        let page = match (req.id.as_deref(), req.rel_path.as_deref()) {
            (Some(id), _) if !id.trim().is_empty() => {
                store.get_page(id.trim()).map_err(|e| {
                    Self::error(ErrorCode::INVALID_PARAMS, format!("Page not found: {e}"))
                })?
            }
            (_, Some(rel_path)) if !rel_path.trim().is_empty() => store
                .get_page_by_rel_path(rel_path.trim())
                .map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("knowledge read failed: {e}"),
                    )
                })?
                .ok_or_else(|| {
                    Self::error(
                        ErrorCode::INVALID_PARAMS,
                        format!("No knowledge page at '{}'", rel_path.trim()),
                    )
                })?,
            _ => {
                return Err(Self::error(
                    ErrorCode::INVALID_PARAMS,
                    "knowledge read requires 'id' or 'rel_path'",
                ));
            }
        };

        let body = store.read_body(&page.rel_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Page body unreadable ({}): {e}", page.rel_path),
            )
        })?;

        let sources = if page.sources.is_empty() {
            "(none)".to_string()
        } else {
            page.sources.join(", ")
        };

        Ok(Self::success(format!(
            "[{}] {} ({})\npath: {}\nlocked: {}\nsources: {}\nupdated: {}\n\n{}",
            page.id,
            page.title,
            page.page_type,
            page.rel_path,
            page.locked,
            sources,
            page.updated_at.to_rfc3339(),
            truncate_str(&body, MAX_BODY_CHARS),
        )))
    }

    /// Write a page by hand.
    ///
    /// A hand-written page is user-sovereign, so it always ends up `locked=1`:
    /// `commit_ingest` refuses to update a locked row (`WHERE locked = 0`) and
    /// deliberately never sets `locked` itself, so the only honest way through
    /// is unlock → write → lock, restoring the previous lock state if the write
    /// fails. Distillation can never overwrite the result.
    pub async fn knowledge_write(
        &self,
        Parameters(req): Parameters<KnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let body = req.body.as_deref().unwrap_or("");
        if body.trim().is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "knowledge write requires a non-empty 'body'",
            ));
        }

        let store = self.open_knowledge_store()?;
        let page_type = req
            .page_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(DEFAULT_PAGE_TYPE)
            .to_string();

        // Path resolution: an explicit rel_path targets an existing page;
        // otherwise the canonical type+title path is the merge key, exactly as
        // in the distillation pipeline.
        let (rel_path, title) = match req.rel_path.as_deref().map(str::trim).filter(|p| !p.is_empty())
        {
            Some(rel_path) => {
                let existing = store.get_page_by_rel_path(rel_path).map_err(|e| {
                    Self::error(
                        ErrorCode::INTERNAL_ERROR,
                        format!("knowledge write failed: {e}"),
                    )
                })?;
                let title = req
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(ToString::to_string)
                    .or_else(|| existing.as_ref().map(|page| page.title.clone()))
                    .ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_PARAMS,
                            format!("No page at '{rel_path}' — pass 'title' to create one"),
                        )
                    })?;
                (rel_path.to_string(), title)
            }
            None => {
                let title = req
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| {
                        Self::error(
                            ErrorCode::INVALID_PARAMS,
                            "knowledge write requires 'title' (or 'rel_path' of an existing page)",
                        )
                    })?;
                (canonical_rel_path(&page_type, title), title.to_string())
            }
        };

        let existing = store.get_page_by_rel_path(&rel_path).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge write failed: {e}"),
            )
        })?;

        let id = match &existing {
            Some(page) => page.id.clone(),
            None => store.generate_id().map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("could not allocate a page ID: {e}"),
                )
            })?,
        };
        let was_locked = existing.as_ref().is_some_and(|page| page.locked);

        // Provenance: caller-supplied paths, else whatever the page already
        // cited, else the manual marker. Never empty — an empty sources list on
        // an unlocked page is what makes the cascade delete it.
        let sources = req
            .sources
            .as_deref()
            .map(parse_sources)
            .filter(|list| !list.is_empty())
            .or_else(|| {
                existing
                    .as_ref()
                    .map(|page| page.sources.clone())
                    .filter(|list| !list.is_empty())
            })
            .unwrap_or_else(|| vec![MANUAL_SOURCE.to_string()]);

        let snippet = req
            .snippet
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| snippet_from_body(body));

        let mut page = KnowledgePage::new(id.clone(), page_type, title);
        // `new` derives a canonical path; an explicit rel_path overrides it so
        // an existing page is updated in place rather than forked.
        page.rel_path = rel_path.clone();
        page.snippet = snippet;
        page.sources = sources;
        if let Some(previous) = &existing {
            page.created_at = previous.created_at;
        }

        if was_locked {
            store.set_locked(&id, false).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("could not unlock page for rewrite: {e}"),
                )
            })?;
        }

        let batch = IngestBatch {
            pages: vec![PageWrite {
                page,
                body: body.to_string(),
            }],
            ..IngestBatch::default()
        };

        let report = match store.commit_ingest(&batch) {
            Ok(report) => report,
            Err(e) => {
                // Leave the page exactly as locked as we found it.
                if was_locked {
                    let _ = store.set_locked(&id, true);
                }
                return Err(Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("knowledge write failed: {e}"),
                ));
            }
        };

        if report.pages_written == 0 {
            if was_locked {
                let _ = store.set_locked(&id, true);
            }
            return Err(Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge write wrote no page for '{rel_path}'"),
            ));
        }

        store.set_locked(&id, true).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("page written but could not be locked: {e}"),
            )
        })?;

        // Resource-change notification is emitted by the `knowledge` tool
        // dispatcher, in line with every other mutating meta-tool.

        Ok(Self::success(format!(
            "{} knowledge page [{id}] {rel_path} (locked: true — distillation will not overwrite it)",
            if existing.is_some() {
                "Updated"
            } else {
                "Created"
            }
        )))
    }

    /// List distilled pages — the injectable index of the wiki.
    pub async fn knowledge_list(
        &self,
        Parameters(req): Parameters<KnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_knowledge_store()?;
        let pages = store.list_pages().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge list failed: {e}"),
            )
        })?;

        if pages.is_empty() {
            return Ok(Self::success(
                "No distilled pages yet. Run `cas knowledge build` to distill the repo.".to_string(),
            ));
        }

        let limit = req.limit.unwrap_or(50).clamp(1, 500);
        let total = pages.len();
        let mut out = format!("Knowledge pages ({total}):\n");
        for page in pages.iter().take(limit) {
            out.push_str(&format!("\n{}", page_line(page)));
            if !page.snippet.is_empty() {
                out.push_str(&format!("\n    {}", page.snippet));
            }
        }
        if total > limit {
            out.push_str(&format!("\n\n... and {} more", total - limit));
        }
        Ok(Self::success(out))
    }

    /// Store health: page counts, lock counts and source-ledger state.
    pub async fn knowledge_status(
        &self,
        Parameters(_req): Parameters<KnowledgeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.open_knowledge_store()?;
        let pages = store.list_pages().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge status failed: {e}"),
            )
        })?;
        let sources = store.list_sources().map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("knowledge status failed: {e}"),
            )
        })?;

        let locked = pages.iter().filter(|page| page.locked).count();
        let pending = pages.iter().filter(|page| page.pending_embedding).count();

        let mut out = format!(
            "Knowledge store: {}\n  pages:   {} ({locked} locked, {pending} awaiting embedding)\n  sources: {}",
            store.knowledge_dir().display(),
            pages.len(),
            sources.len(),
        );
        for status in [
            cas_store::SourceStatus::Ingested,
            cas_store::SourceStatus::Uploaded,
            cas_store::SourceStatus::Failed,
        ] {
            let count = sources.iter().filter(|row| row.status == status).count();
            if count > 0 {
                out.push_str(&format!("\n    {}: {count}", status.as_str()));
            }
        }
        Ok(Self::success(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_skips_frontmatter_headings_and_provenance_markers() {
        let body = "---\ntitle: Hooks\n---\n\n# Hooks\n\n<!-- cas:sources [\"docs/hooks.md\"] -->\n> quoted\nThe hook dispatcher fans events out to handlers.\n";
        assert_eq!(
            snippet_from_body(body),
            "The hook dispatcher fans events out to handlers."
        );
    }

    #[test]
    fn snippet_of_a_body_with_nothing_but_headings_is_empty() {
        assert_eq!(snippet_from_body("# Title\n\n## Section\n"), "");
    }

    #[test]
    fn sources_are_split_and_trimmed_and_blanks_dropped() {
        assert_eq!(
            parse_sources(" docs/a.md , ,docs/b.md "),
            vec!["docs/a.md".to_string(), "docs/b.md".to_string()]
        );
        assert!(parse_sources("  ,  ").is_empty());
    }
}
