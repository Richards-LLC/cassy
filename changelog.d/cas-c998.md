### Fixed — Merge queue

- **Each failed merge-candidate retry reports its failure** — When preparing a merge candidate failed and you retried the merge, the retry said preparation "continues in the background", but its failure notice was dropped as a duplicate of the first one, so the merge stayed held with no further word. Now each failed attempt sends its own failure notice (GH #2106).
