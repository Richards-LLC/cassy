---
name: cas-why
description: Use when asked why code was built a certain way, what a line or module was for, or whether a design choice was deliberate; answers with confidence tiers and a coverage map.
metadata:
  managed_by: cas
---

# Why was it built this way

Answer a "why" question about code from recorded evidence, say how sure each
claim is, and list every source you checked, including the ones that found
nothing. An absent record is a finding: it tells the reader the reason was
never written down, which is different from a reason you guessed.

## Procedure

1. **Pin the subject.** Name the path, symbol or line range and the current
   commit (`git rev-parse --short HEAD`). Write the question as one sentence.
   Done when the subject resolves to real code at that commit.
2. **Gather evidence**, running each source once and recording its hit count
   for the coverage map:
   - `search action=blame file_path="<path>:<start>-<end>" include_prompts=true`
     for who wrote each line and the session and prompt behind it.
   - `search action=history path="<path>" include_provenance=true` (add
     `symbol=<name>` for one symbol, `include_merges=true` when a task's only
     commit may be a merge). Read `index_status` and `unsupported`: a stale or
     partial index lowers what a zero means. A commit returned with an unlinked
     `reason` counts as a commit without a task link.
   - `search action=search query="<symbol or concept>" doc_type=task`, then
     `task action=show` and `task action=notes id=<id>` for each linked task:
     descriptions and `decision` notes are the strongest evidence.
   - `search action=search query="<symbol or concept>" doc_type=entry` for
     memories, and `doc_type=spec` for specs.
   - The commit messages and pull requests of the commits found
     (`git show -s <sha>`, `gh pr list --state merged --search <sha>`).
   - Comments and docs beside the code: the file's own comments, then
     `search action=grep pattern="<symbol>" glob="docs/**"` for design notes.
   Stop at about 15 minutes; list the sources not reached in the coverage map.
3. **Classify every claim** into one tier:

   | Tier | Standard |
   | --- | --- |
   | Documented | A commit message, PR, task, decision note, spec or code comment states the reason. Cite it. |
   | Inferred | No source states it, but two or more pieces of evidence point the same way (a pattern across commits, a test that pins the behaviour, a sibling module built the same way). Name the evidence. |
   | Unknown | No evidence either way. Say so; do not fill the gap with a plausible reason. |

   One source that only restates what the code does is not a reason; it stays
   Unknown.
4. **Answer** in this order: the one-line answer with its tier, the supporting
   claims with tiers and citations, then the coverage map. Done when every
   claim carries a tier and every source from step 2 appears in the map.

## Coverage map

List each source checked with its query and hit count. Keep 0-hit rows; they
are the evidence that a reason was never recorded.

| Source | Query | Hits | Note |
| --- | --- | --- | --- |
| blame | `src/auth.rs:40-80` | 3 sessions | 2 lines from one prompt |
| history | `path=src/auth.rs` + provenance | 7 commits | 2 unlinked (no task) |
| tasks | `"token refresh"` | 1 | decision note states the reason |
| memories | `"token refresh"` | 0 | no recorded learning |
| specs | `"token refresh"` | 0 | |
| PRs | commits above | 1 | description repeats the task |
| docs/comments | `"refresh_token"` in `docs/**` | 0 | |
| not reached | none | | |

If the question matters for a decision and the answer is Unknown, record the
finding with `memory action=remember` once the owner confirms the real reason,
so the next `why` finds it.
