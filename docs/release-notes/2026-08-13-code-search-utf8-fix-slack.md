# Release notes — code search UTF-8 crash fix (main, 2026-08-13)

Channel: #cas-internal (C0B44GUKDK2)
Merge: 5ab9a412 to main (pushed to origin/main), CI green.

## User thread

**Top-level:**
Live on production · **User** — Code search no longer crashes when results contain box-drawing characters, emoji, or other non-ASCII text.

**Reply:**
Was → searching code whose matches landed near box-drawing characters, emoji, or CJK text could kill the search with a hard crash instead of returning results.
Now → result snippets are trimmed safely at character boundaries, so searches over any file content come back clean every time.

## Dev thread

**Top-level:**
Live on production · **Dev** — code_search snippet truncation is now UTF-8-boundary-safe.

**Reply:**
Was → snippet truncation in the code-search response path sliced strings at raw byte offsets, panicking whenever the cut landed inside a multi-byte character (box-drawing, emoji, CJK).
Now → truncation clamps to the nearest character boundary; a regression suite drives repeated box-drawing/emoji/CJK sources through `CodeSearch::search`, and an audit confirmed no byte-indexed string slicing remains anywhere in that response path. Landed as a red-test-first commit pair; search suite 115/115 green.

## POSTED

- 2026-08-13T14:00Z (UTC), channel #cas-internal (C0B44GUKDK2)
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786629528013639
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786629539124829?thread_ts=1786629528.013639&cid=C0B44GUKDK2
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786629549006649
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786629561305319?thread_ts=1786629549.006649&cid=C0B44GUKDK2
