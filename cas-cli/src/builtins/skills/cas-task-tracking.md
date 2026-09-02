---
name: cas-task-tracking
description: Use when work needs persistent Cassy task tracking, dependencies, progress notes, or cross-session continuity.
managed_by: cas
---

# Cassy Task Tracking

Use `mcp__cas__task` instead of built-in TodoWrite. Cassy tasks persist across sessions.

## Core Workflow

1. **Create**: `mcp__cas__task action=create title="..." description="..." priority=2`
2. **Start**: `mcp__cas__task action=start id=<task-id>`
3. **Progress**: `mcp__cas__task action=notes id=<task-id> notes="..." note_type=progress`
4. **Close**: `mcp__cas__task action=close id=<task-id> reason="..."`

## Useful Actions

- **Ready tasks**: `mcp__cas__task action=ready` — unblocked, actionable work
- **My tasks**: `mcp__cas__task action=mine` — tasks assigned to you
- **Blocked**: `mcp__cas__task action=list status=blocked`
- **Add dependency**: `mcp__cas__task action=dep_add id=<task> to_id=<blocker> dep_type=blocks`

## Note Types

`progress`, `blocker`, `decision`, `discovery`, `question` — use the right type so notes are meaningful in context.

## Valid Actions

The list below is the dispatch order for `mcp__cas__task`; keep it synchronized with the live service.

**Valid `mcp__cas__task` actions** (exact list — do not invent others): `create`, `proposal_inbox`, `proposal_accept`, `proposal_reject`, `proposal_reconcile`, `show`, `update`, `start`, `close`, `cancel`, `reopen`, `request_changes`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `reset`, `transfer`, `available`, `mine`.
