---
name: cas-search
description: Use when you need to find Cassy context, code, a symbol, a file pattern, or a prior task, memory, rule, or skill.
metadata:
  managed_by: cas
---

# Cassy Search

Use `search` to find information across Cassy content and code.

## Which Action to Use

| Need | Call |
|---|---|
| Conceptual match across memories, tasks, rules, skills, specs, artifacts and indexed code | `search action=search query="authentication flow" doc_type=entry` |
| Code symbols by what they do | `search action=code_search query="user authentication" kind=function language=rust` |
| Exact regex in indexed files | `search action=grep pattern="TODO:" glob="*.rs"` |
| Who wrote lines, linked to the AI session and prompt | `search action=blame file_path="src/auth.rs:40-80" ai_only=true` |
| Indexed git commit history | `search action=history query="..." path=... since=...` |
| Session or delegated-task context | `search action=context` or `context_for_subagent`, with `task_id` and `max_tokens` |
| A symbol's source by indexed ID | `search action=code_show id=<symbol-id> include_source=true` |

`search` filters: `doc_type` is `entry`, `task`, `rule`, `skill`, `spec`, `artifact`, `code_symbol` or `code_file` (an unrecognized value searches every type); `scope` is `global`, `project` or `all` (default); `tags` is comma-separated and every tag must match.

To answer why code was built a certain way, use the `cas-why` skill: it combines `blame`, `history` and task and memory search into a tiered answer with a coverage map.

Less common actions: `retrieval_feedback` (`query_id`, `result_id`, `outcome`, `actor_id`, plus `correction_ref` for `corrected`), `retrieval_metrics` (optional `session_id`), `skill_impact` (alias `impact_report`), `observe` (`content`, `observation_type`, `source_tool`), and `entity_list`/`entity_show`/`entity_extract` for knowledge-graph entities.

## Structured Memory Filters

For memories with structured frontmatter embedded in their `content` (see `cas-memory-management`), queries accept inline filters that AND with keyword terms:

`search action=search query="deadlock module:cas-mcp severity:critical"`

Recognized keys are `module`, `track`, `problem_type`, `severity`, `root_cause` and `date`. Unknown `key:value` tokens remain keyword text. Values cannot contain whitespace; quoting and escaping are not supported.

## Valid Actions

**Valid `search` actions** (exact list): `search`, `retrieval_feedback`, `retrieval_metrics`, `skill_impact`, `impact_report`, `context`, `context_for_subagent`, `observe`, `entity_list`, `entity_show`, `entity_extract`, `code_search`, `code_show`, `grep`, `blame`, `history`.
