# Search manifest — T3 Code chat rendering

Task: cas-b4e8. Read-only source study on October 28, 2026.

## Snapshot and trust boundary

- Canonical source: [pingdotgg/t3code](https://github.com/pingdotgg/t3code); `exa-search --contents` returned repository metadata (homepage t3.codes, MIT, default main) and README. README independently links to the same T3 Code product; this confirms the requested repository identity, not publisher identity beyond GitHub.
- `git ls-remote --symref https://github.com/pingdotgg/t3code.git HEAD` returned `refs/heads/main` and `5211acfe95a0c6d82a60e2524b5e46b47fe67a3e`.
- Fresh clone: `/mnt/rewind/cas/scratch/cas-b4e8-t3code`, `git clone --depth=1 --single-branch ...`.
- Clone `show -s --format='%H%n%cI%n%s'`: same SHA, `2026-10-28T06:02:48-07:00`, `feat(web): boring sidebar can show folder threads on two lines (#26463)`.
- No upstream commands other than Git metadata/fetch, filename listings, `rg`, `head`, `nl` and `sed`. No installs, builds, scripts, tests, package-manager commands or application execution. No upstream instruction files were followed.
- Commander baseline: `25624dd2f2d011c515ddb4314b2541032e2faec2`.

## Upstream content read

Paths are relative to the pinned T3 Code root. Excerpt reads establish only the indicated scope; filename matches do not count as content inspection.

| File | Read scope / purpose |
| --- | --- |
| `README.md` | Lines 1–35; identity and product links |
| `LICENSE` | All 21 lines; MIT notice |
| `apps/web/src/components/ChatView.tsx` | Searches for timeline/tool/reasoning; render path 3205–3276 |
| `apps/web/src/components/ChatMarkdown.tsx` | Pattern searches; excerpts 360–400, 929–1005, 1500–1683; headings, alerts, tables, code, streaming boundary |
| `apps/web/src/components/chat/ThreadTimeline.tsx` | 1–57; active timeline path and isolated live text |
| `apps/web/src/components/chat/MessagesTimeline.tsx` | Import/call searches and 1476–1535; active row dispatch |
| `apps/web/src/components/chat/MarkdownCodeBlock.tsx` | 1–250, 275–372; frame, actions, budgets, streaming, cache/fallback |
| `apps/web/src/components/chat/MarkdownTable.tsx` | All 71 lines; scroll surface, copy formats |
| `apps/web/src/components/chat/TimelineWorkEntryRow.tsx` | Searches plus 118–140, 278–352, 388–455, 514–558; reasoning, typed tool body and failure |
| `apps/web/src/components/chat/TimelineWorkGroupRows.tsx` | Searches plus 270–310, 350–420, 600–631; working/thinking state, groups |
| `apps/web/src/components/chat/TimelineToolActivityIcon.tsx` | Symbol searches; icon vocabulary only |
| `apps/web/src/components/chat/TimelineRowParts.tsx` | Symbol searches; author/timestamp/shimmer entry points only |
| `apps/web/src/components/chat/WorkLog.tsx` | 1–165; row density and live-edge following |
| `apps/web/src/components/chat/TimelineAssistantRows.tsx` | Symbol searches and 288–372; changed-file section |
| `apps/web/src/components/diffs/StyledDiffCodeView.tsx` | 1–160; Pierre adapter, split sizing, selection, headers |
| `apps/web/src/components/media/AttachmentImage.tsx` | All 63 lines; decode fallback and cleanup |
| `apps/web/src/components/chat/ExpandedImageDialog.tsx` | Searches plus 104–145, 200–245; navigation, failure, zoom |
| `apps/web/src/components/chat/MarkdownMermaidCodeBlock.tsx` | All 84 lines; source/diagram toggle and completion gate |
| `apps/web/src/markdown-incremental.ts` | All 98 lines; cached prefix and fallbacks |
| `apps/web/src/lib/incrementalHighlighting.ts` | Symbol searches and 48–108; stable lines and tokenizer fallback |
| `apps/web/src/index.css` | Relevant pattern searches; 530–580, 630–660, 1886–1925, 2197–2240; motion, type and tables |

Searches included `rg --files apps/web/src/components` with message/timeline/chat/markdown/tool/reasoning/diff/attachment/theme filters; `rg -n` for stream, table, Shiki, cache, code, alerts, reasoning, duration, lifecycle, copy, image navigation and reduced motion. Output was narrowed to relevant files; file lists included tests but no upstream tests were executed or relied on as passing evidence. An initial directory listing before clone completion returned “no such file”; the clone later completed successfully. A guessed `chat/ChangedFilesCard.tsx` path did not exist; the report cites the verified caller instead.

## Commander content read

| File | Read scope / purpose |
| --- | --- |
| `DESIGN.md` | Entire document; report branding and inheritance; last change b43940e43 |
| `hub-web/DESIGN.md` | Entire document plus numbered excerpts 240–285; semantic roles and behavior; last change ce0d8adc9 |
| `hub-web/src/markdown-renderer.ts` | Entire file; safe dialect, fence handling, heading flattening |
| `hub-web/src/thread-model.ts` | 1–110, 340–415; grouping, work, evidence tables and cell tones |
| `hub-web/src/conversation-view.ts` | Pattern searches and 40–100, 1285–1335, 1360–1425, 1685–1738, 1800–1885; dispatch, content, working, media reuse and anchors |
| `hub-web/src/work-events.ts` | Entire file; event/detail schema and presentation helpers |
| `hub-web/src/work-rows-view.ts` | Entire file; disclosures, detail states, task chips and focus preservation |
| `hub-web/src/rich-code.ts` | All code; dispatch and source fallback |
| `hub-web/src/rich-diff.ts` | Entire file; hunk rendering and lazy disclosure |
| `hub-web/src/rich-attachment.ts` | Relevant symbol/line search; viewport and preview affordances |
| `hub-web/src/rich-media.ts` | Relevant symbol/line search; lazy loading and media types |
| `hub-web/src/rich-diagram.ts` | Searches plus 20–end; bounded/sanitized diagram and source disclosure |
| `hub-web/src/rich-pdf.ts` | Filename only; PDF dispatch was read in rich-media, not this implementation |
| `hub-web/src/rich-content.css` | Entire file; diagram/diff/media styling |
| `hub-web/src/fonts.css` | Entire file; local font faces |
| `hub-web/public/cassy-tokens.css` | Scheme/font role searches and inlined by trusted finalizer; no copied second palette |
| `hub-web/public/favicon.svg` | Read by trusted finalizer for embedded brand mark |

## Report production evidence

Trusted local tools: Python markdown-it renders the Markdown source into a draft with `/*cas:brand*/`; `cas deliverable finalize` inlines the repository brand and creates/checks A4 and Letter PDFs. Strict visual-QA checks the self-contained report in light/dark, desktop/phone, print and JavaScript-off. The final QA receipt and PDF page receipts live in the task artifact directory; the small visual receipt is committed beside the report. Rendering helpers remain task artifacts. The report contains no upstream application assets or code.

Additional workflow references read: cas-worker (+details, discipline, close-gate), cas-html-reports (+report-types, technical-contract, presentation-rules, review-checklist), cas-ui-craft (+concept-brief, form-vocabulary, critique-rubric), DESIGN.md-first, exa-search, verify-before-claim and cas-release-report PDF instructions. These are workflow inputs, not evidence about T3 Code.
