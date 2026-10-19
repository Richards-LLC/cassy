# CAS_ROOT override notice: readable store choice

| Field | Decision |
| --- | --- |
| First two lines | Lead with the CAS_ROOT winner; keep both short paths on that line when they fit, otherwise follow the verdict with labeled complete paths. |
| Scannable | Keep warning/ASCII marks, order the overriding store before the working-directory store, and indent continuation rows. |
| Readable | Wrap at the actual terminal cell width; long paths use a backslash at the continuation boundary, without dropping characters, and the remedy stays on its own copyable line. |
| Machine output | The notice remains stderr-only and at most once per process; stdout scalars and JSON shapes are unchanged. |
| Omitted | No path truncation, new flags, colors, store routing or precedence changes; wrapped path continuations can be joined to recover the exact original path. |

## Critique

Run1 dc5f3661b had16 overflow findings for the intended notice: first line300
cells and fixed remedy line191 at80/120. Installed3.64/d17444d has the same
unwrapped remedy; see cas-30c1 TRIAGE art-b83dde0f. The revised candidate's
terminal gate and light/dark critique are supervisor-owned in the assembled
flow walk (4200505), not claimed by source tests.
