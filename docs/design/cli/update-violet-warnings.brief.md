# cas update Violet advisories

| Field | Contract |
| --- | --- |
| First two lines | A Violet warning names the skipped profile or advisory and its cause; builtin sync continues. |
| Scannable | Each warning wraps at whitespace with its retry or verification command on a separate line; path tokens never split. |
| Readable | HOME paths use `~`; over-wide path tokens keep their first segment and leaf around a middle ellipsis, with a one-line `--verbose` hint. |
| Machine output | The existing update JSON document adds `warnings` containing complete absolute warning paths; compact diagnostics stay on stderr, and `--verbose` prints full human paths. |
| Omitted | Credential values, parser error chains and repeated full HOME prefixes stay off compact output; stored advisory paths and trust boundaries remain unchanged. |

A shared-base symlink beneath the active HOME is already trusted. An explicit
profile outside HOME does not authorize a sibling target outside both roots;
the warning correctly describes that refused target. This includes inherited
profile selectors during an isolated-HOME update. The renderer leaves this
boundary intact and names a retry after the operator reviews the skipped path.


## Critique

`terminal-qa: PASS cas-36cd · 12 runs · 0 fail · 0 warn · 0 allowed`

Supervisor candidate `eae6fee69890fa7f201493d2561bc3f063bbe63b`,
`cas 3.72.0 (eae6fee 2026-10-22)`, SHA-256
`7c81bf7b6162aa05627c114c6369b36e8e205fbc8de6ab3524281ede18aa1887`.
The actual CLI passed five isolated-HOME cells: human continuation, explicit
profile boundary, JSON full warnings, verbose full path tokens, and accepted
inside-HOME shared base. Full captures are task cas-36cd `terminal2/`; the gate
uses `--escape-flag --verbose`, matching the displayed command. The earlier
word-split failure is retained as `art-4e771130`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Each scoped warning names the migration/advisory and cause, followed by a separate retry/remedy line. |
| Fit | 5 | HOME contraction and middle-truncated whole path tokens fit every 80/120 capture without word splitting. |
| Craft | 4 | Each shortened warning has one Full paths command; valid shared bases emit no boundary warning. |
| Theme safety | 5 | Four palettes, pipe and NO_COLOR pass; C locale uses ASCII dots instead of the ellipsis glyph. |
| Machine contract | 5 | JSON warnings keep complete absolute paths, and verbose human output preserves full path tokens. |

Scored by young-otter-20 on 2026-10-22 against supervisor captures. The existing
progress-row spacing defect is outside this warning renderer and is being
filed separately by the supervisor; this task does not change that row.
