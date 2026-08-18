---
managed_by: cas
---

# Memory Overlap Detection

Run this workflow **before** creating a new memory via `cas__memory action=remember`. It catches the case where the agent is about to write a second memory about a problem Cassy already has captured — the silent drift cause that the refresh workflow has to clean up later.

The cheapest fix is to never write the duplicate. The next cheapest is to write it with an explicit cross-reference. Both are this workflow's job.

## When to run this check

Run the check on every `remember` call **except**:

- Bulk imports (`--no-overlap-check` flag) — overlap detection adds latency that bulk operations can't afford. The refresh workflow will catch any duplicates later.
- Agent-internal scratch memories that explicitly opt out (rare).

The check should add no more than ~1–2s of latency for typical memory creation. Keep it fast: extract → search → score → decide.

## The 4 steps

### 1. Extract key terms from the new memory

From the new memory's title, description, and body, extract:

- **Title tokens** — significant words from `name`, lowercased, stop words removed.
- **Symptom/error strings** — quoted error text, function names, file paths.
- **Module / tags** — `module` and `tags` frontmatter fields if present.
- **Reference symbols** — file paths, function/class/module names, commit SHAs.

Build a query string for BM25 search: prefer reference symbols (most discriminating), then symptom strings, then title tokens. Drop pure prose.

### 2. Search existing memories

Call `cas__search` with the query string and `doc_type=entry` (memory entries). Take the top **3–5 candidates** by score. If the top result has a score below the search engine's "weak match" threshold, treat the candidate set as empty and skip to step 4 (create normally).

If the new memory has a `module` field, prefer candidates from the same module — they're far more likely to be true overlaps. Boost same-module candidates one rank.

### 3. Score overlap across 5 dimensions

For each candidate, score 0 or 1 on each dimension. Total score is 0–5.

| Dimension | Match signal |
|---|---|
| **Problem statement** | Same problem described in `description` / Problem section / `name`. Compare semantically — wording will differ; concepts should match. |
| **Root cause** | Same `root_cause` enum value, OR the body's diagnosed cause maps to the same underlying mechanism. |
| **Solution approach** | Same fix shape — same file edited, same config flag flipped, same pattern applied. Different wording is fine; same intervention is the signal. |
| **Referenced files** | Significant overlap in cited files/symbols. "Significant" = 2+ shared references, or 1 shared reference if it's the central file in both memories. |
| **Tags** | 2+ tag matches, OR 1 tag match if it's a highly specific tag (e.g. `wal`, `ntfs`, `sqlite-locking`). Generic tags like `bug` or `mcp` don't count. |

**Scoring rules:**

- Score conservatively. When unsure, mark 0, not 1.
- A candidate's `module` mismatch is a strong negative signal — if modules differ, automatically subtract 1 from the final score (floor at 0).
- A candidate marked `status: stale` still counts. Stale memories are exactly the ones that need to be replaced rather than duplicated.
- A `track` mismatch (bug vs knowledge) is also a strong negative — subtract 1 from the final score. A bugfix and a best practice on the same area are usually distinct.

### 4. Decide based on the highest-scoring candidate

| Score | Action |
|---|---|
| **4–5** (high overlap) | **Do not create.** Surface the existing memory, ask whether to (a) update the existing memory in place, or (b) skip if the new content adds nothing. In autofix/headless mode, update in place. |
| **2–3** (moderate overlap) | **Create with cross-reference.** Add `related_memories: [<existing-slug>]` to the new memory's frontmatter. Add an inline "Related: <existing-slug>" line in the body. Also add the new slug to the existing memory's `related_memories` array (bidirectional link). |
| **0–1** (low/no overlap) | **Create normally.** No links. |

If multiple candidates score in the 2–3 range, cross-link to all of them (cap at 3 links — beyond that, the links become noise and the situation calls for a refresh/consolidate run).

If two or more candidates score 4–5, that is a smell: the existing memory set already has duplicates. Surface all of them, recommend running `cas memory refresh` after creation, and pick the most recent one as the update target.

## High-overlap update flow

When score ≥ 4 and the user confirms updating the existing memory:

1. **Preserve the original `date`.** Don't overwrite it.
2. **Add `updated: YYYY-MM-DD`** (today) to frontmatter.
3. **Merge content selectively:**
   - Keep the **most specific** information from both versions. If the new content names a more precise function/file, use it.
   - If the new content adds a symptom not in the existing `symptoms` array, append (respect 5-item max).
   - If the new content corrects a wrong claim in the existing memory, **replace** the wrong claim, don't keep both — that defeats the purpose.
   - If the new content adds a "What Didn't Work" or "Prevention" bullet that the existing memory lacks, append it.
4. **Don't churn.** If the new content is just a reword of the existing memory, skip the update — return the existing memory unchanged with a note.
5. **Update `MEMORY.md` index** if the `description` line changed.

## Cross-reference flow (moderate overlap)

When score is 2–3:

1. Create the new memory normally.
2. Add the candidate slug(s) to its `related_memories` frontmatter array.
3. Add a line at the end of the new memory body: `## Related\n- <slug-1>: <one-line description>`.
4. Open each candidate and append the new slug to its `related_memories` array. If the candidate has no `## Related` section, add one. If it has one, append a bullet.
5. Don't otherwise edit the candidates — cross-linking is a one-line bidirectional change, not an excuse to refresh the candidate.

## Edge cases

### Stale candidates

If the highest-scoring candidate has `status: stale` and the new memory describes the *current* approach to the same problem, this is a **Replace**, not an Update or a new memory:

1. Treat the new memory as the successor.
2. Add `supersedes: <stale-slug>` to the new memory's frontmatter.
3. Delete the stale candidate file.
4. Update `MEMORY.md` index: remove old entry, add new entry.
5. This is conceptually the same as the Replace flow in the refresh workflow — the only difference is the trigger (proactive on remember vs reactive on refresh).

### New memory has explicit `supersedes`

If the agent calling `remember` has already declared `supersedes: <slug>` in the new memory's frontmatter, skip the overlap check entirely — the agent has already made the decision. Trust it.

### Module mismatch with high score

If a candidate scores 4–5 but is from a different module, this is suspicious. Either:

- The new memory's `module` field is wrong → ask before creating.
- The two modules genuinely share the issue (e.g., both depend on the same library) → create with a cross-reference even though the score is high.

In autofix mode, prefer the cross-reference path — don't auto-update across modules.

### No `module` field on either side (legacy memories)

Legacy memories without `module` are common. Run the check anyway; just skip the module-mismatch penalty. The other 4 dimensions still work.

### The new memory IS just the existing memory rewritten

If score is 5 and the new content adds literally nothing new (same problem, same solution, same files, same tags, same root cause), skip creation entirely and return the existing memory's slug to the caller. Don't even update — there's nothing to update.

## Modes

### Interactive

When score ≥ 4: present the existing memory with a one-paragraph diff summary and ask:

```
A very similar memory already exists:

  <existing-slug>
  <existing-description>

Overlap: 5/5 (problem, root cause, solution, files, tags)

What would you like to do?
1. Update the existing memory (recommended)
2. Skip — the new content adds nothing
3. Create the new memory anyway (will create a duplicate)
```

When score is 2–3: do not ask. Create with cross-references and report what was linked.

### Autofix / headless

- Score 4–5: update in place, no question.
- Score 2–3: create with cross-references, no question.
- Score 0–1: create normally.

The autofix path must always succeed in either creating or updating something — never silently skip.

## Implementation notes (for the future Rust path)

When this check moves into Rust (`cas-core::memory::overlap`), the steps map cleanly:

1. **Term extraction** — pure Rust. Tokenize the new memory, drop stop words, extract symbol-shaped tokens (CamelCase, snake_case, paths, file extensions). Tokenizer already exists in the search index path.
2. **BM25 search** — already implemented. Reuse `cas__search`'s underlying call. Pass the extracted terms as the query.
3. **Dimension scoring** — implementable as pure Rust heuristics. The structured frontmatter fields from cas-559d (`root_cause`, `module`, `tags`) make most dimensions cheap exact-match comparisons. Problem-statement and solution-shape comparisons need either token-overlap heuristics or a small embedding model — start with token overlap and upgrade only if precision suffers.
4. **Decision** — table lookup based on score.
5. **Side effects** — file edits for cross-reference / update flows. Use the existing memory write path; add a `mode: update | create-with-link | create` enum to the writer.

Performance budget: total overlap check should run in <500ms for a memory store with 10k entries. The bottleneck is BM25 search; everything after the candidate set is small.

Add a `--no-overlap-check` flag to `cas__memory action=remember` for bulk imports and tests that intentionally create overlapping memories.

## Relationship to the refresh workflow

Overlap detection is **proactive**: catch duplicates at creation time so the memory set never accumulates them.

The refresh workflow is **reactive**: clean up duplicates that have already accumulated (legacy memories, memories created with `--no-overlap-check`, memories where the overlap check missed a true duplicate).

Both are needed. Overlap detection misses things — semantically identical memories with no shared symbols, memories whose problem domains converged after creation, memories created while the search index was stale. Refresh is the safety net.

When overlap detection's high-overlap path triggers and the user picks "create anyway", that is a strong signal to run refresh on that module soon. Consider logging it.
