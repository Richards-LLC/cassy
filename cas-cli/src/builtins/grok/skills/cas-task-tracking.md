---
name: cas-task-tracking
description: How to track work using CAS tasks instead of built-in TodoWrite. Use for persistent task tracking with priorities, dependencies, structured notes, and cross-session continuity.
managed_by: cas
---

# CAS Task Tracking

Use `cas__task` instead of built-in TodoWrite. CAS tasks persist across sessions.

## Core Workflow

1. **Create**: `cas__task action=create title="..." description="..." priority=2`
2. **Start**: `cas__task action=start id=<task-id>`
3. **Progress**: `cas__task action=notes id=<task-id> notes="..." note_type=progress`
4. **Close**: `cas__task action=close id=<task-id> reason="..."`

## Useful Actions

- **Ready tasks**: `cas__task action=ready` — unblocked, actionable work
- **My tasks**: `cas__task action=mine` — tasks assigned to you
- **Blocked**: `cas__task action=list status=blocked`
- **Add dependency**: `cas__task action=dep_add id=<task> to_id=<blocker> dep_type=blocks`

## Note Types

`progress`, `blocker`, `decision`, `discovery` — use the right type so notes are meaningful in context.

## Valid Actions

**Valid `cas__task` actions** (exact list — do not invent others): `create`, `show`, `update`, `start`, `close`, `reopen`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `transfer`, `available`, `mine`.
