# duplicate-detector job

Find and consolidate duplicate memory entries to keep the knowledge base lean.

## Input

The queued job prompt lists the entry IDs for this run (at most 15). Process exactly those IDs; do not fetch a different set. This is a bounded one-shot job with no task of its own, so it cannot file task notes.

## Process

For each entry ID from the queued prompt:

1. **Read:** `mcp__cas__memory action=get id=<id>`.
2. **Search for duplicates:** `mcp__cas__search action=search query="<key phrases from the content>" limit=5`.
3. **Classify each match:**
   - **Exact:** same content, different IDs. Archive the older one.
   - **Near or semantic:** same topic and meaning, different wording, or one is a subset of the other. Merge into the more complete entry, keeping the clearer phrasing.
   - **Complementary:** same topic, each with unique information. Merge both into one entry.
   - **Not a duplicate:** similar topic but different conclusions or scope. Leave both.
4. **Consolidate confirmed pairs only:** update the kept entry with `mcp__cas__memory action=update id=<keep> content="<merged>"`, then archive the other with `mcp__cas__memory action=archive id=<dup>`.

## Output

End with one line per case you did not merge because you were unsure:

`UNCERTAIN <keep-id> <dup-id>: <reason>`

Print nothing else for those cases; a human reads these lines from the job log.

## Guidelines

- Preserve all unique information when merging; never lose knowledge. Union the tags.
- Keep the entry with higher importance, more helpful feedback, or the more recent timestamp.
- Do not merge across scopes: global and project entries may legitimately overlap.
- Do not merge entries with different conclusions about the same topic; they may record evolving understanding.
