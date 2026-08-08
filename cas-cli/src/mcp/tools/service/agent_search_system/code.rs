use crate::mcp::tools::service::imports::*;

fn build_git_blame_command(file_spec: &str) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    // Use `--` so a path like `--help` is treated as a file path, not a git flag.
    command.args(["blame", "--porcelain", "--", file_spec]);
    command
}

fn parse_blame_file_spec(file_spec: &str) -> (String, Option<usize>, Option<usize>) {
    let Some((path, suffix)) = file_spec.rsplit_once(':') else {
        return (file_spec.to_string(), None, None);
    };

    if path.is_empty() {
        return (file_spec.to_string(), None, None);
    }

    if let Ok(line) = suffix.parse::<usize>() {
        return (path.to_string(), Some(line), Some(line));
    }

    if let Some((start, end)) = suffix.split_once('-') {
        if let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) {
            let (start, end) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            return (path.to_string(), Some(start), Some(end));
        }
    }

    (file_spec.to_string(), None, None)
}

impl CasService {
    pub(in crate::mcp::tools::service) async fn code_search_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_code::{Language, SymbolKind};
        use cas_search::CodeSearchOptions;

        let query = req.query.unwrap_or_default();
        if query.is_empty() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                "query required for code_search",
            ));
        }

        let cas_root = &self.inner.cas_root;
        if !crate::hybrid_search::code::code_search_available(cas_root) {
            return Ok(Self::success(
                serde_json::to_string_pretty(&serde_json::json!({
                    "results": [],
                    "message": "Code search index not found. Run `cas index code` to index your codebase."
                }))
                .unwrap(),
            ));
        }

        let code_search =
            crate::hybrid_search::code::open_code_search_fast(cas_root).map_err(|e| {
                Self::error(
                    ErrorCode::INTERNAL_ERROR,
                    format!("Failed to open code search: {e}"),
                )
            })?;

        let kind: Option<SymbolKind> = req.kind.as_ref().and_then(|value| value.parse().ok());
        let language: Option<Language> = req.language.as_ref().and_then(|value| value.parse().ok());

        let opts = CodeSearchOptions {
            query: query.clone(),
            limit: req.limit.unwrap_or(10),
            kind,
            language,
            include_source: req.include_source.unwrap_or(false),
            min_score: 0.0,
            // Capability-gated by `CasCodeSearch`: logged-out installs keep
            // this request entirely local while authenticated installs merge
            // the isolated code-vector channel with BM25.
            semantic: true,
        };

        let results = code_search.search(&opts).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Code search failed: {e}"),
            )
        })?;

        let results_json: Vec<serde_json::Value> = results
            .iter()
            .map(|result| {
                serde_json::json!({
                    "id": result.id,
                    "name": result.name,
                    "kind": format!("{:?}", result.kind),
                    "language": format!("{:?}", result.language),
                    "file_path": result.file_path,
                    "line_start": result.line_start,
                    "line_end": result.line_end,
                    "score": result.score,
                    "snippet": result.snippet,
                    "documentation": result.documentation,
                    "source": result.source,
                })
            })
            .collect();

        let response = serde_json::json!({
            "query": query,
            "count": results.len(),
            "results": results_json,
        });

        Ok(Self::success(
            serde_json::to_string_pretty(&response).unwrap(),
        ))
    }

    pub(in crate::mcp::tools::service) async fn code_show_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        let id = req
            .id
            .ok_or_else(|| Self::error(ErrorCode::INVALID_PARAMS, "id required for code_show"))?;

        let cas_root = &self.inner.cas_root;
        let code_store = crate::store::open_code_store(cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open code store: {e}"),
            )
        })?;

        let symbol = code_store.get_symbol(&id).map_err(|e| {
            Self::error(ErrorCode::INTERNAL_ERROR, format!("Symbol not found: {e}"))
        })?;

        let response = serde_json::json!({
            "id": symbol.id,
            "name": symbol.name,
            "qualified_name": symbol.qualified_name,
            "kind": format!("{:?}", symbol.kind),
            "language": format!("{:?}", symbol.language),
            "file_path": symbol.file_path,
            "file_id": symbol.file_id,
            "line_start": symbol.line_start,
            "line_end": symbol.line_end,
            "source": if req.include_source.unwrap_or(true) { Some(&symbol.source) } else { None },
            "documentation": symbol.documentation,
            "signature": symbol.signature,
            "parent_id": symbol.parent_id,
            "repository": symbol.repository,
        });

        Ok(Self::success(
            serde_json::to_string_pretty(&response).unwrap(),
        ))
    }

    /// Grep search using ripgrep libraries.
    pub(in crate::mcp::tools::service) async fn grep_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use cas_search::{GrepOptions, GrepSearch};

        let pattern = req.pattern.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "pattern required for grep action",
            )
        })?;

        let cas_root = &self.inner.cas_root;
        let search_root = cas_root.parent().unwrap_or(cas_root);

        let grep = GrepSearch::new(search_root);
        let limit = req.limit.unwrap_or(100).min(1000);
        let results = grep
            .search(&GrepOptions {
                pattern,
                glob: req.glob.clone(),
                before_context: req.before_context.unwrap_or(0),
                after_context: req.after_context.unwrap_or(0),
                case_insensitive: req.case_insensitive.unwrap_or(false),
                limit,
            })
            .map_err(|error| {
                Self::error(ErrorCode::INTERNAL_ERROR, format!("Grep failed: {error}"))
            })?;

        let mut output = String::new();
        for item in &results {
            let before_start = item
                .line_number
                .saturating_sub(item.before_context.len() as u64);
            for (index, line) in item.before_context.iter().enumerate() {
                output.push_str(&format!(
                    "{}:{}-{}\n",
                    item.file_path,
                    before_start + index as u64,
                    line
                ));
            }
            output.push_str(&format!(
                "{}:{}:{}\n",
                item.file_path, item.line_number, item.line_content
            ));
            for (index, line) in item.after_context.iter().enumerate() {
                output.push_str(&format!(
                    "{}:{}-{}\n",
                    item.file_path,
                    item.line_number + 1 + index as u64,
                    line
                ));
            }
            if !item.before_context.is_empty() || !item.after_context.is_empty() {
                output.push_str("--\n");
            }
        }
        output.push_str(&format!("\n[{} matches]\n", results.len()));

        Ok(Self::success(output))
    }

    /// Git blame with AI session attribution.
    pub(in crate::mcp::tools::service) async fn blame_impl(
        &self,
        req: SearchContextRequest,
    ) -> Result<CallToolResult, McpError> {
        use std::collections::HashMap;

        use crate::store::{open_commit_link_store, open_prompt_store};

        let file_spec = req.file_path.ok_or_else(|| {
            Self::error(
                ErrorCode::INVALID_PARAMS,
                "file_path required for blame action",
            )
        })?;
        let (resolved_file_path, parsed_line_start, parsed_line_end) =
            parse_blame_file_spec(&file_spec);
        let line_start = req.line_start.or(parsed_line_start);
        let line_end = req.line_end.or(parsed_line_end);

        if !std::path::Path::new(&resolved_file_path).exists() {
            return Err(Self::error(
                ErrorCode::INVALID_PARAMS,
                format!("File not found: {resolved_file_path}"),
            ));
        }

        let output = build_git_blame_command(&resolved_file_path)
            .output()
            .map_err(|e| {
                Self::error(ErrorCode::INTERNAL_ERROR, format!("Failed to run git: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("git blame failed: {stderr}"),
            ));
        }

        let blame_content = String::from_utf8_lossy(&output.stdout);
        let blame_lines = parse_git_blame_porcelain(&blame_content);

        let cas_root = &self.inner.cas_root;
        let commit_link_store = open_commit_link_store(cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open store: {e}"),
            )
        })?;
        let prompt_store = open_prompt_store(cas_root).map_err(|e| {
            Self::error(
                ErrorCode::INTERNAL_ERROR,
                format!("Failed to open store: {e}"),
            )
        })?;

        let unique_commits: Vec<_> = blame_lines
            .iter()
            .map(|line| line.commit_hash.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let mut commit_links: HashMap<String, cas_types::CommitLink> = HashMap::new();
        for commit_hash in &unique_commits {
            if let Ok(Some(link)) = commit_link_store.get(commit_hash) {
                commit_links.insert(commit_hash.clone(), link);
            }
        }

        let mut attributions: Vec<serde_json::Value> = Vec::new();
        for line in blame_lines {
            if let Some(start) = line_start {
                if line.line_number < start {
                    continue;
                }
            }
            if let Some(end) = line_end {
                if line.line_number > end {
                    continue;
                }
            }

            let commit_link = commit_links.get(&line.commit_hash).or_else(|| {
                commit_links
                    .iter()
                    .find(|(hash, _)| {
                        hash.starts_with(&line.commit_hash) || line.commit_hash.starts_with(*hash)
                    })
                    .map(|(_, value)| value)
            });

            let is_ai_generated = commit_link.is_some();
            if req.ai_only.unwrap_or(false) && !is_ai_generated {
                continue;
            }

            if let Some(ref filter_session) = req.session_id {
                if commit_link.map(|link| &link.session_id) != Some(filter_session) {
                    continue;
                }
            }

            let (session_id, agent_id, prompt_id, prompt_snippet) = if let Some(link) = commit_link
            {
                let prompt = link
                    .prompt_ids
                    .first()
                    .and_then(|prompt_id| prompt_store.get(prompt_id).ok().flatten());

                let include_full = req.include_prompts.unwrap_or(false);
                let snippet = prompt.as_ref().map(|prompt| {
                    let content = &prompt.content;
                    if include_full {
                        content.clone()
                    } else {
                        truncate_str(content, 100)
                    }
                });

                (
                    Some(link.session_id.clone()),
                    Some(link.agent_id.clone()),
                    prompt.as_ref().map(|prompt| prompt.id.clone()),
                    snippet,
                )
            } else {
                (None, None, None, None)
            };

            attributions.push(serde_json::json!({
                "line_number": line.line_number,
                "content": line.content,
                "commit_hash": line.commit_hash,
                "author": line.author,
                "is_ai_generated": is_ai_generated,
                "session_id": session_id,
                "agent_id": agent_id,
                "prompt_id": prompt_id,
                "prompt_snippet": prompt_snippet,
            }));
        }

        Ok(Self::success(
            serde_json::to_string_pretty(&attributions).unwrap(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{build_git_blame_command, parse_blame_file_spec};

    #[test]
    fn blame_command_uses_separator_before_file_path() {
        let cmd = build_git_blame_command("--help");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();

        assert_eq!(args, vec!["blame", "--porcelain", "--", "--help"]);
    }

    #[test]
    fn parse_blame_file_spec_parses_single_line() {
        let (path, line_start, line_end) = parse_blame_file_spec("src/main.rs:42");
        assert_eq!(path, "src/main.rs");
        assert_eq!(line_start, Some(42));
        assert_eq!(line_end, Some(42));
    }

    #[test]
    fn parse_blame_file_spec_parses_line_range() {
        let (path, line_start, line_end) = parse_blame_file_spec("src/main.rs:10-20");
        assert_eq!(path, "src/main.rs");
        assert_eq!(line_start, Some(10));
        assert_eq!(line_end, Some(20));
    }

    #[test]
    fn parse_blame_file_spec_leaves_plain_path_unchanged() {
        let (path, line_start, line_end) = parse_blame_file_spec("src/main.rs");
        assert_eq!(path, "src/main.rs");
        assert!(line_start.is_none());
        assert!(line_end.is_none());
    }
}
