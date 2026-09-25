---
name: cas-search
description: Use when you need to find Cassy context, code, a symbol, a file pattern, or a prior task, memory, rule, or skill.
metadata:
  managed_by: cas
---

# Cassy Search

Use `search` to find information across Cassy content and code. Choose the right action for the job:

## Which Action to Use

**`search`** — conceptual queries across memories, tasks, rules, skills, and indexed code:
`search action=search query="authentication flow" doc_type=entry`
Filter with `doc_type`: `entry`, `task`, `rule`, `skill`, `spec`, `artifact`,
`code_symbol`, or `code_file`; an unrecognized value searches every type.
`scope` is `global`, `project`, or `all` (default); `tags` is comma-separated
and every tag must match.

**`context`** — session context summary. Use `task_id` to focus the result on one task and `max_tokens` to bound it.

**`context_for_subagent`** — task-focused context for a delegated worker; pass `task_id` and `max_tokens`.

**`code_search`** — find code symbols by what they do, not only by exact names. Use `kind`, `language`, and `include_source` when useful:
`search action=code_search query="user authentication" kind=function language=rust`

**`grep`** — exact regex matching in indexed files. Use `pattern`, optionally `glob`, `before_context`, `after_context`, and `case_insensitive`:
`search action=grep pattern="TODO:" glob="*.rs"`

**`blame`** — git blame for one file, linked to the AI sessions and prompts that wrote each line. Pass `file_path` (optionally `path:line` or `path:start-end`); add `line_start`/`line_end`, `ai_only`, or `include_prompts` as needed:
`search action=blame file_path="src/auth.rs:40-80" ai_only=true`

**`history`** — search the indexed git commit history. Use `query`, optionally `path`, `symbol`, `since`, `until`, and `include_merges`; every response includes index freshness information.

**`retrieval_feedback`** — record an outcome for a provenance result. Pass `query_id`, `result_id`, `outcome`, and `actor_id`; add `correction_ref` when the outcome is `corrected`.

**`retrieval_metrics`** — aggregate recorded retrieval outcomes. An optional `session_id` limits the report.

**`skill_impact`** — report skill usage and session outcomes. `impact_report` is an accepted alias for this action.

**`observe`** — record an observation with `content`, `observation_type`, `source_tool`, and optional `tags`/`scope`.

**`entity_list`**, **`entity_show`**, **`entity_extract`** — browse or derive knowledge-graph entities. Use `id` for `entity_show`; use `entity_type`, `query`, `tags`, `scope`, and `limit` as applicable.

**`code_show`** — show a code symbol by its indexed symbol ID; use `include_source` for source text.

## Structured Memory Filters

For memories with structured frontmatter embedded in their `content` (see `cas-memory-management`), search queries support inline filters that AND with keyword terms:

`search action=search query="deadlock module:cas-mcp severity:critical"`
`search action=search query="track:bug problem_type:runtime_error"`

Recognized filter keys are `module`, `track`, `problem_type`, `severity`, `root_cause`, and `date`. Unknown `key:value` tokens remain keyword text. Values cannot contain whitespace; quoting and escaping are not supported.

## Valid Actions

**Valid `search` actions** (exact list — do not invent others): `search`, `retrieval_feedback`, `retrieval_metrics`, `skill_impact`, `impact_report`, `context`, `context_for_subagent`, `observe`, `entity_list`, `entity_show`, `entity_extract`, `code_search`, `code_show`, `grep`, `blame`, `history`.
