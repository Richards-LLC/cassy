---
name: cas-improve-architecture
description: Use when explicitly asked to scan architectural hot spots, compare module-deepening candidates and work through a chosen improvement; not for an individual bug diagnosis or an already chosen module design.
disable-model-invocation: true
license: MIT
metadata:
  managed_by: cas
  author: Matt Pocock
  upstream: https://github.com/mattpocock/skills
  provenance: Adapted from improve-codebase-architecture (MIT, © 2026 Matt Pocock).
---

# Improve architecture

Imported and adapted from mattpocock/skills `improve-codebase-architecture`,
MIT © 2026 Matt Pocock. See [LICENSE](LICENSE) for the permission notice.

Run on explicit invocation. Find substantial behavior that can live behind a
smaller interface, then work through the selected candidate. A report is the
selection surface, not evidence that a refactor already shipped.

## Procedure

1. Read `cas-codebase-design` for depth, locality, seams and dependency
   categories; established framework vocabulary wins. Bind the scan to the
   requested paths, symptoms and revision. Otherwise use recent git history
   and frequently changed/tested paths. Search project Cassy specs, memories
   and related tasks for domain terms, prior decisions and active improvements.
   Record the scope, examined revision and missing evidence in a task note.
2. Scan actual callers and tests for understanding that requires hopping
   between shallow modules, an interface almost as complex as its implementation,
   coupled internals leaking across seams, or tests that pin source/prose instead
   of exercising behavior. Preserve justified metadata and structural contracts;
   a textual test alone does not prove the module is shallow.
3. Delegate read-only exploration of the hot spots to sub-agents with distinct
   scopes and the same revision. Ask for file:line evidence, actual callers,
   interface facts, hidden complexity, and a deletion test: does removing the
   current module simply remove a pass-through, or spread complexity to callers?
   For each proposed consolidation, test the converse: would deleting the new
   module spread that complexity again? Collect disagreements and counter-evidence.
   If delegation is unavailable, record the gap and perform a second independent
   pass; lower recommendation strength where that gap matters.
4. Classify each candidate's dependencies: in-process, local-substitutable,
   owned remote (ports and adapters), or true external (injected mock adapter).
   Prefer an interface tested directly with real in-process behavior or a local
   stand-in; introduce a varying seam only for justified implementations.
5. Produce a practitioner decision brief through `cas-html-reports`: markdown
   source, concept brief and offline HTML together. Give every candidate its
   files and evidence, problem, proposed interface, before/after diagrams,
   deletion-test result, dependency category, benefits and trade-offs. Mark
   strength as Strong, Worth exploring or Speculative and name one top
   recommendation with the deciding criterion. Label diagrams of proposals as
   planned. Include prior decisions contradicted and what would falsify the
   recommendation; avoid presenting source inference as executed proof.
6. Work through the candidate chosen by the invoking operator or supervisor.
   Use the existing authorization to resolve routine details. If the choice is
   unspecified, present the recommendation for selection; a worker records it
   for the supervisor without starting another task. Clarify constraints,
   failure cases, dependencies, interface facts and the behavior tests that
   replace obsolete internal checks. For a consequential interface, hand off
   to `cas-codebase-design`'s design-it-twice comparison before implementing.
7. Search for overlapping tasks by cause and mechanism. Attach the candidate
   to an existing match, or create a scoped task with evidence, constraints,
   observable acceptance and the replacement test surface. Follow the active
   role's assignment policy; creating a task does not authorize starting it.
   Record the chosen interface and rejected alternatives in a decision note,
   durable design trade-offs with `spec action=create`, and settled project
   terms or discoveries with `memory action=remember`; refine existing entries
   instead of creating a competing glossary or architecture-record directory.

Done when the candidate report exists, selection or its pending owner is
recorded, and the chosen improvement has a deduplicated task and decision
record. Implementation is complete only when that task's acceptance is proven.
