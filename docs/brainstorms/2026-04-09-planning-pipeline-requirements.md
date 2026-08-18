---
date: 2026-04-09
topic: planning-pipeline-implementation-unit-template
---

# Planning Pipeline: Implementation Unit Template for EPIC Specs

## Problem Frame

Cassy supervisors write EPIC task specs freeform. Two supervisors planning the same EPIC produce structurally different specs, and workers must parse each one individually to figure out what is in scope, which files they own, what "done" looks like, and what tests matter. The current `cas-supervisor` skill at `cas-cli/src/builtins/skills/cas-supervisor.md` is already substantial (~300 lines) with adversarial posture, intake gates, planning gates, trajectory gates, review gates, and spec requirements — but its `Spec Requirements` section lists *what must be present* (acceptance criteria, interface, layer boundary, non-goals, test guidance) without specifying *how the prose is structured*. The result is compliant specs that are still hard to scan at a glance.

The compound engineering roadmap §1.3.2 specifies an Implementation Unit template that names the structure explicitly: Goal, Requirements, Dependencies, Files, Approach, Execution note, Patterns, Test scenarios, Verification. The cas-6103 task cas-2ccd drafted the template but never shipped it. The audit (cas-1a2e) confirmed no cas-supervisor extension around this was built.

This subsystem ships the template as a skill-content addition only — no Rust work, no schema changes, no new skills. Confidence-check deepening (§1.3.3) and document review personas (§2.2) are **deferred** to separate future EPICs. The primary value is that every EPIC subtask has a predictable shape a worker can parse in five seconds.

## Requirements

**Template Definition**
- **R1.** The Implementation Unit template is documented as a new section in `cas-cli/src/builtins/skills/cas-supervisor.md` (and the codex mirror) with the following structure:
  ```markdown
  - [ ] **Unit N: [Name]**

  **Goal:** What this unit accomplishes
  **Requirements:** R1, R2      # only when an EPIC brainstorm doc exists
  **Dependencies:** None | Unit X | cas-<id>
  **Files:**
    - Create: `path/to/new_file.rs`
    - Modify: `path/to/existing_file.rs`
    - Test: `path/to/test_file.rs`
  **Approach:** Key design or sequencing decision
  **Execution note:** test-first | characterization-first | additive-only | (omit)
  **Patterns to follow:** Reference existing code to mirror
  **Test scenarios:**
    - Happy path: input X -> expected Y
    - Edge case: empty input -> returns error Z
    - Error path: network failure -> retries 3x then fails
  **Verification:** Observable outcomes when complete
  ```
- **R2.** Each field has a short documented purpose in the skill, so supervisors know what to write and what NOT to write. Specifically: the template captures *decisions*, not pre-written implementation code. "Approach" is 1–3 sentences of sequencing and design choice, not a code sketch. "Files" lists paths only, not intended diffs.
- **R3.** `Execution note` references the `execution_note` field added in subsystem F (cas-worker extensions). The template and the task field share the same enum — `test-first`, `characterization-first`, `additive-only`, or omitted. When the supervisor sets it on the task via `mcp__cas__task action=update execution_note=...`, the template's Execution note line should match.

**Mapping to Task Fields**
- **R4.** The template's fields map to existing `mcp__cas__task` schema as follows:
  | Template field | Maps to |
  |---|---|
  | Unit N name | `title` |
  | Goal | first paragraph of `description` |
  | Requirements | prose bullet in `description` (convention, not a new field) |
  | Dependencies | `blocked_by` (for hard blocks) or `description` prose (for soft deps) |
  | Files | `description` prose block |
  | Approach | `design` field |
  | Execution note | `execution_note` field (subsystem F) |
  | Patterns to follow | `description` prose |
  | Test scenarios | `acceptance_criteria` field |
  | Verification | `demo_statement` field |
- **R5.** No new task schema fields are added by this subsystem. The template is entirely prose-in-description convention plus use of existing fields (`design`, `acceptance_criteria`, `demo_statement`, `blocked_by`, and the subsystem-F-added `execution_note`).

**Scope of Application**
- **R6.** The template applies **only to tasks that are EPIC subtasks** — specifically `task_type=task` or `task_type=feature` that are children of an EPIC via parent-child dependency. Standalone bugs, chores, and spikes are explicitly out of scope and continue to be freeform.
- **R7.** Spike tasks (`task_type=spike`) continue to use their existing question-based acceptance criteria format, not the Implementation Unit template. A spike's deliverable is understanding, not implementation, so the template doesn't fit.
- **R8.** The two smallest Phase 1 EPICs (F: cas-worker extensions, E: this subsystem) ship without retroactively re-specifying already-dispatched tasks. Existing open tasks are not migrated.

**Requirements Traceability**
- **R9.** When an EPIC has an originating brainstorm requirements doc at `docs/brainstorms/YYYY-MM-DD-<topic>-requirements.md`, the EPIC description should reference the doc path once, and each subtask's `Requirements:` line lists the R-IDs it satisfies (e.g., `Requirements: R3, R4, R5`). This is convention only — no new field, no validation.
- **R10.** R-IDs are stable: when a subtask's Requirements line lists `R3, R4`, it must refer to the R3/R4 of the linked brainstorm doc, not locally renumbered. The existing cas-brainstorm skill's `references/requirements-capture.md` already enforces stable IDs, so this is an alignment rule, not a new constraint.

**Enforcement**
- **R11.** Enforcement is **skill guidance only** — cas-supervisor skill documents the template as mandatory for EPIC subtasks, but no Rust code validates task descriptions against it. Supervisors follow the convention like they already follow the other ~300 lines of adversarial posture and gate rules. No `task.create` validation, no warnings, no hard blocks.
- **R12.** The template documentation explicitly notes that individual fields can be marked `N/A` or omitted when a unit genuinely doesn't need them (e.g., a cosmetic skill-content edit has no meaningful `Test scenarios` beyond "content renders cleanly"). The intent is structure, not ceremony.

**Integration**
- **R13.** A brief pointer to the template is added to the existing `Spec Requirements` section of cas-supervisor.md, cross-linking to the new template section. The two sections complement each other: `Spec Requirements` enumerates *what must be present*; the Implementation Unit template specifies *how the prose is shaped*.

## Success Criteria

- **Primary:** every EPIC subtask created after this ships follows the template. Verified by scanning `mcp__cas__task action=list task_type=task` output for tasks with EPIC parents and confirming the `description` field contains Goal / Files / Test scenarios / Verification sections.
- Workers spend measurably less time clarifying scope at task start — fewer "what do you mean by X" questions to the supervisor during the first 10 minutes of execution. (Subjective, no metric, but the supervisor will notice.)
- The Phase 1 EPIC for subsystem A (multi-persona review) is dispatched using the template — it is the first real test of whether the template fits a large EPIC.
- Zero regression in current cas-supervisor skill behavior: nothing in the existing ~300 lines is modified or removed, only added to.

## Scope Boundaries

- **Not shipping:** confidence-check deepening (§1.3.3). Separate future EPIC with its own brainstorm. That work adds a "supervisor self-scores the spec and dispatches targeted research before handoff" pre-dispatch gate; it's additive to this subsystem and doesn't need to land together.
- **Not shipping:** document review personas (§2.2). A `cas-doc-review` skill with coherence / feasibility / scope-guardian / adversarial personas reviewing EPIC specs. Structurally similar to subsystem A's multi-persona code review but for plans. Deferred to its own EPIC.
- **Not shipping:** automated template validation in Rust. No `task.create` handler changes, no warnings, no hard blocks.
- **Not shipping:** retroactive migration of existing open tasks. Current tasks stay freeform until closed.
- **Not shipping:** requirements-doc ↔ task link as a real data relationship. It's a prose convention.
- **Not shipping:** a new skill file. Content lives inside the existing `cas-supervisor.md`.
- **Not changing:** the cas-brainstorm skill, its `references/requirements-capture.md`, or the existing stable R-ID format.
- **Not changing:** the cas-supervisor-checklist skill. It remains the startup checklist; the implementation unit template is reference material within the main cas-supervisor skill.
- **Not changing:** how EPICs themselves are described. Only subtasks under EPICs are in scope. EPIC parent descriptions stay freeform.

## Key Decisions

- **Skill-only enforcement, no Rust work.** Rationale: cas-supervisor is already a ~300-line prose skill of rules; adding one more section is consistent. Mechanical enforcement would be disproportionate to the benefit.
- **Append to cas-supervisor.md, don't restructure.** Rationale: converting the flat file to a directory skill (like cas-brainstorm) is a real migration with BuiltinFile registration changes and sync-composition risk. Not worth it for one new section.
- **EPIC subtasks only, not universal.** Rationale: spikes, bugs, and chores don't fit the "decisions + test scenarios + verification" framing. Forcing the template on them is ceremony for its own sake.
- **No new task fields.** Rationale: the existing schema (`description`, `design`, `acceptance_criteria`, `demo_statement`, `blocked_by`, `execution_note` from subsystem F) covers all the template fields with convention in `description` for the rest. Adding structured fields is subsystem-F-sized work for a documentation benefit.
- **Requirements traceability via convention, not link.** Rationale: brainstorm docs are version-controlled markdown in the same repo as the supervisor session; a prose "R1, R2" reference plus a doc path at the EPIC level is enough for grep-based traceability. A real link would require schema changes for marginal lookup benefit.
- **Confidence-check and doc-review deferred.** Rationale: the user's success signal was "EPIC specs consistently structured" — confidence-check and doc-review are secondary. Splitting them into their own EPICs keeps Brainstorm E small and shippable.
- **No retroactive migration.** Rationale: the template targets future tasks created by supervisors who have read the updated skill. Retrofitting old tasks is busywork with no operational value.

## Dependencies / Assumptions

- **Dependency on subsystem F (cas-worker extensions).** The `execution_note` field in the template maps to the new task field shipped in subsystem F. If subsystem F is not yet shipped when this EPIC lands, the `Execution note` template line is documented but the field it refers to doesn't exist yet. Planning must either sequence subsystem F before E, or document the field as "coming from subsystem F" in the template until it lands.
- **Assumption: supervisors reading the updated cas-supervisor skill will use the template.** The skill is already a heavy discipline document; supervisors are assumed to follow it. No enforcement mechanism, so compliance is trust-based.
- **Assumption: the cas-brainstorm skill's `references/requirements-capture.md` remains the source of truth for stable R-ID format.** This subsystem references it but does not duplicate it.

## Outstanding Questions

### Resolve Before Planning

*(empty — all product-level decisions made in this brainstorm)*

### Deferred to Planning

- **[Affects R1, R13]** [Technical] Exact placement of the new template section in the existing cas-supervisor.md. Current structure has Hard Rules → Adversarial Posture (Intake / Planning / Trajectory / Spec Requirements / Assignment Checks / Review Gates / Ongoing Discipline) → Worker Modes → Worker Count Strategy → Workflow → Valid Actions → Schema Cheat Sheet. The template likely lands between "Spec Requirements" and "Assignment Checks" or as a subsection of "Spec Requirements." Planning picks placement.
- **[Affects R1]** [Technical] Line-count budget for the new section. Prior cas-2ccd drafted roughly 40 lines; accepting some expansion for field-level guidance and examples, budget ~60-80 lines of markdown added to cas-supervisor.md. Planning reviews the draft before committing.
- **[Affects R4, R5]** [Needs research] Current state of `mcp__cas__task` schema — specifically whether `design`, `acceptance_criteria`, and `demo_statement` are all still first-class fields in the MCP API and the Task struct. The mapping table in R4 assumes they are. Planning verifies.
- **[Affects R3]** [Dependency] Sequencing relative to subsystem F (cas-worker extensions). If F ships first, the Execution note line references a live field. If E ships first, the line references a field that doesn't exist yet. Planning sequences and notes any interim wording.
- **[Affects R11]** [Needs research] Whether the cas-supervisor skill's `managed_by: cas` frontmatter works with appended content — i.e., `cas sync` overwrites cleanly on updates. Phase 0 verified this for the distribution mechanism, but planning should re-verify specifically for an edit that grows an existing file rather than adding a new one.
- **[Affects R9]** [Technical] Does the EPIC description have a standard place for the brainstorm doc path reference, or is it ad-hoc? Planning establishes one convention for EPIC-level doc references in the skill.

## Next Steps

→ Hand off to planning (cas-supervisor). Plan a Phase 1 EPIC scoped to subsystem E: add the Implementation Unit template section to `cas-supervisor.md` (claude + codex mirrors), cross-link from `Spec Requirements`, no Rust work.

This is deliberately the smallest subsystem in the Phase 1 program aside from (possibly) operational-polish work. The entire EPIC is 2 markdown-file edits, a pointer in another section, and verification that the synced skill renders correctly to workers. Ship after subsystem F so the `Execution note` line references a real field.

Implementation can draw heavily on the template already drafted in the closed task cas-2ccd's description — the content needs only minor reconciliation with this document's decisions (EPIC-subtasks-only scope, convention-not-field R-IDs, skill-only enforcement).
