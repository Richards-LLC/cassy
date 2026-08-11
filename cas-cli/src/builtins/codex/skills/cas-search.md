---
name: cas-search
description: Use when you need to find CAS context, code, a symbol, a file pattern, or a prior task, memory, rule, or skill.
managed_by: cas
---

# CAS Search

Use `mcp__cs__search` to find information across CAS content and code. Choose the right action for the job:

## Which Action to Use

**`search`** — Conceptual queries across memories, tasks, rules, skills:
```
mcp__cs__search action=search query="authentication flow" doc_type=entry
```
Filter with `doc_type` (entry, task, rule, skill, code_symbol, code_file) for better relevance.

**Structured memory filters (`key:value` tokens):** For memories with structured frontmatter (see `cas-memory-management`), the query string supports inline filters that AND with keyword terms:

```
mcp__cs__search action=search query="deadlock module:cas-mcp severity:critical"
mcp__cs__search action=search query="track:bug problem_type:runtime_error"
```

Recognized filter keys: `module`, `track`, `problem_type`, `severity`, `root_cause`, `date`. Unknown `key:value` tokens pass through as raw keyword text. Legacy memories (no structured frontmatter) still match keyword queries but are excluded from any filter that references a structured field.

Phase 1 grammar limitation: values cannot contain whitespace and there is no quoting or escaping — tokens split on whitespace and the first `:` separates key from value. `module:"cas mcp"` and escaped colons are not supported.

**`code_search`** — Find code symbols by what they do, not exact names:
```
mcp__cs__search action=code_search query="user authentication" kind=function language=rust
```
Use `include_source=true` to get source code inline.

**`grep`** — Exact regex pattern matching in files:
```
mcp__cs__search action=grep pattern="TODO:" glob="*.rs"
```
Prefer the built-in Grep tool for simple patterns where you already know file paths.

## Decision Guide

| Need | Action |
|------|--------|
| "How does X work?" | `search` or `code_search` |
| Find exact string or regex | `grep` |
| Find past learnings | `search` with `doc_type=entry` |
| Find function by concept | `code_search` |
| Find related tasks | `search` with `doc_type=task` |

## Other Actions

- **`context`** — Session context summary: recent activity, active tasks, relevant memories
- **`context_for_subagent`** — Task-focused context for spawning subagents (pass `task_id` and `max_tokens`)
- **`observe`** — Record discoveries/decisions during work (`observation_type`: general, decision, bugfix, feature, refactor, discovery)
- **`entity_list`** / **`entity_show`** — Browse extracted entities (person, project, technology, etc.)
- **`code_show`** — Full details for a specific code symbol by ID
- **`blame`** — Git blame with optional AI-line filtering (`file_path`, `line_start`, `line_end`)

## Valid Actions

**Valid `mcp__cs__search` actions** (exact list — do not invent others): `search`, `context`, `context_for_subagent`, `observe`, `entity_list`, `entity_show`, `entity_extract`, `code_search`, `code_show`, `grep`, `blame`.
