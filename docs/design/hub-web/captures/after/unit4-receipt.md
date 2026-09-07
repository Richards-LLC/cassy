# Unit 4: session working surfaces

Implements the approved [concept brief](../../concept-brief.md), specifically the session,
transcript and attention forms. Captured 2026-09-07 using production styles,
`TranscriptView` and `renderAttentionPanel`; terminal canvases are placeholders.
The terminal emulator, transcript model, ANSI palette and WASM pins are unchanged.

## Evidence

`visual-qa --strict`: **PASS — 0 findings, 16 captures**, light/dark × 1280×800/390×844
for session-canvas, transcript, attention-12 and attention-0. Capture filenames begin
`unit4-<screen>-<scheme>-<width>.png` in this directory. For example:
[phone transcript](unit4-transcript-light-390.png),
[desktop attention](unit4-attention-12-light-1280.png), and
[phone worker rows](unit4-session-canvas-dark-390.png).

Only the documented JavaScript-disabled application loss is allowlisted; no contrast,
clipping, overlap, overflow or invisible-text findings are suppressed. Attention text is
unmuted (no `data-visual-qa-hidden` helpers). The fixture renders twelve distinct events,
including critical, warning and info events, long detail text and a wrapping codename.
Desktop has two expanded panes plus one collapsed worker; phone workers stack as 32px rows.

Fresh browser assertions passed in all four scheme/viewport combinations: transcript
15px/22px and 68ch measure, no horizontal transcript overflow, wide diagrams scroll within
their own keyboard-reachable rows, twelve complete action labels, group collapse/expand,
and phone worker stacking/height. Heading/code/tool-looking text and unfamiliar text are
preserved literally; this reading surface introduces no Markdown parser.

Fresh source checks: `npm run typecheck` exit 0; `npm test` exit 0, 30 files / 428 tests;
`npm run build -- --outDir .unit4-app-build` exit 0. Generated dist remains unchanged.
The two amended invariant assertions remove the old shimmer/clamp contract; all other
existing invariants remain unchanged. Pending enrichment now reads “Enriching…”.

Durable receipt root: `/home/pippenz/.cas/artifacts/cas-3c1c8/`:

- `visual-mixed/visual-qa.md` and `visual-qa.json`: strict receipt and screenshots.
- `browser-behavior.json` and `browser-behavior.mjs`: layout and interaction proof.
- `tests-final.log`, `typecheck-final.log`, `app-build.log`: source checks.
- `fixture-extension.patch`: fixture follow-up against Unit 6 branch commit
  `028c2cc18d5bacab05d517f4a9e0429226e2e1af` (fixture contents at capture time).

Supervisor instruction 27202 accepts source plus these artifacts now; the fixture patch
is integration-owned after Unit 6 merges. It preserves FIXTURE_NAMES and leaves fleet,
pairing and connection fixture behavior intact. Reproduce by applying the patch after
Unit 6, building its fixture entry with Vite, and invoking `runVisualQa` from
`scripts/visual-qa.mjs` on the four URLs with the sizes above. The shared runner and final
print sheet are integration-owned. These captures establish the working surfaces;
live terminal output and the finished fleet shell are outside this receipt.

## Scoped critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Warm chrome and dark terminal wells frame a ruled attention timeline with one indigo critical mark. |
| Fit | 4 | Full event causes lead directly to one recovery action; transcript preserves machine-authored text in a reading column. |
| Hierarchy | 4 | Critical event leads the timeline; timestamps and codenames are quieter mono metadata, without count-badge piles. |
| Craft | 4 | Strict matrix has zero findings; desktop wells and stacked phone worker rows retain readable chrome. |
| Accessibility | 4 | All measured text clears contrast; actions stay visible, diagrams accept keyboard focus, and groups reopen after collapse. |

Scored by mighty-newt-96, 2026-09-07. Scoped floor holds. Unit 7 owns the assembled-page
critique and print validation after all screen units merge.
