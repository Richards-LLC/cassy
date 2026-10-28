# Make the content richer before adding more chrome

<figure class="hero-figure" aria-labelledby="hero-caption">
<div class="pair">
<div class="sample"><p class="eyebrow">Today · source-derived schematic</p><pre><code>const ready = true;</code></pre><p class="sample-note">Plain fence. Select text to copy.</p></div>
<div class="sample proposed"><p class="eyebrow">Proposed · technique 01</p><div class="code-label"><span>TypeScript</span><span>Copy · Wrap</span></div><pre><code><span class="syntax">const</span> ready = <span class="syntax">true</span>;</code></pre><p class="sample-note">Recognise it. Copy it. Read it on a phone.</p></div>
</div>
<figcaption id="hero-caption">Same authored example, two information hierarchies. Diagram, not a product screenshot or working controls. Based on Commander’s fence renderer and T3 Code’s code frame at the revisions below; inspected October 28, 2026.</figcaption>
</figure>

**Recommendation:** adopt labeled code frames first, then syntax highlighting, useful tables, typed tool details and readable reasoning disclosures. Confidence is high in the cited source behavior and medium in the priority order; no usability or speed improvement was measured.

## Sources and scope

| Source | Revision / date | Type and reliability |
| --- | --- | --- |
| T3 Code, canonical `pingdotgg/t3code`, default branch `main` | `5211acfe95a0c6d82a60e2524b5e46b47fe67a3e`; committed October 28, 2026, 9:02:48 AM ET | Primary source snapshot. GitHub repository metadata names t3.codes; its README links to the same product and describes the agent control surface. Remote HEAD and clone HEAD matched. High confidence for this snapshot, not future releases. |
| Commander | `25624dd2f2d011c515ddb4314b2541032e2faec2`; source inspected October 28, 2026 | Primary source in this checkout. The renderer, thread model, work-event model and rich-content modules were read together. No claim about an independently deployed build. |
| Cassy design contract | `DESIGN.md@b43940e43`, inheriting `hub-web/DESIGN.md@ce0d8adc9` | Primary design specification. Existing Glass, violet actions, semantic status colors, Inter and JetBrains Mono remain the baseline. |
| License | T3 Code `LICENSE:1–21` at the pinned SHA | MIT; copyright 2026 T3 Tools Inc. Technique study only; no upstream implementation or artwork copied. |

The upstream was fetched into a fresh directory under `/mnt/rewind/cas/scratch`. Only Git metadata, directory listings, searches and file reads were used there. **No upstream code, package manager, install, build, test, lifecycle hook or repository script was executed.** All diagrams are independently authored explanations. The report renderer and PDF/visual checks execute only trusted local tooling against this report.

The active source path was traced through `ChatView.tsx:3205–3276` → `chat/ThreadTimeline.tsx:14–56` → `chat/MessagesTimeline.tsx:1476–1495`, including its assistant, work, thinking and working rows. This matters because finding an attractive unused component would not establish a product technique.

## Ranked top five for a follow-up epic

Order is a design judgment: immediate reading benefit, reuse of existing data, then implementation dependency. It is not a measured score. Estimates assume one engineer familiar with Commander, including focused tests and accessibility review: **S = 1–2 engineering days; M = 3–5; L = 6–10+**. They exclude product approval and cross-provider rollout; rows overlap and must not be summed as a schedule.

| Rank | Follow-up slice | Effort | User-visible completion check |
| --- | --- | --- | --- |
| 1 | **Code frames:** language/title, Copy and Wrap; technique 01 | S | Copy returns the exact source; an unknown language stays readable; long lines fit a 390px viewport without page overflow. |
| 2 | **Lazy syntax highlighting:** technique 02, on top of frames | M | Language tokens remain readable in both schemes; failed highlighting leaves the source visible; selection survives a repaint. |
| 3 | **Tables worth using:** inline formatting, correct cells and Markdown/CSV copy; technique 03 | M | A table with escaped pipes, inline code and a long path reads correctly and copies into a spreadsheet without losing columns. |
| 4 | **Typed tool details:** technique 04; schema proposal before cards | L overall; M UI once fields exist | Read/search/command/edit each have a useful summary and an inspectable body; unknown tools retain their raw payload; failure is named. |
| 5 | **Reasoning summaries and disclosures:** technique 05 | M for published summaries | A published thinking summary is readable inline and opens its detail without moving the reader or resetting disclosure state. No hidden model reasoning is inferred or requested. |

Suggested boundary: one renderer epic with two tracks. Frames, highlighting and tables use current message data. Tool semantics require a versioned work-event extension, provider provenance and a plain fallback; do that contract before UI. Reasoning uses only the summary/detail already intentionally published by the harness. The work below is a proposal, not an implementation commitment.

## Twelve concrete techniques

### 01 · Give every code fence a small control surface

**T3 Code does:** a bordered code frame carries a fence filename or language, file/language icon, Copy feedback and a wrap toggle. See [apps/web/src/components/chat/MarkdownCodeBlock.tsx:44–129](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/MarkdownCodeBlock.tsx#L44-L129) and its actions at lines 154–239. This is the opening diagram’s source idea.

**Commander gap:** [hub-web/src/markdown-renderer.ts:263–274](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/markdown-renderer.ts#L263-L274) creates a plain `pre/code`; only Mermaid/diff/patch receive enhancement. Ordinary fence language is read but not displayed in a frame. **Benefit:** recognise the snippet and copy exact content without dragging a selection. **S · Adopt · rank 1.** Implement as safe DOM around existing source, with touch-visible actions and honest copy failure feedback. Skip upstream’s command-run action in this slice: it changes execution authority and is unrelated to reading.

### 02 · Color syntax, with bounded work and a source fallback

**T3 Code does:** Shiki-backed highlighting, a completed-block LRU capped at 500 entries and 50 MiB, and a line-preserving path for streamed blocks. It deliberately avoids replacing streamed lines with cached HTML when a stream finishes. See [apps/web/src/components/chat/MarkdownCodeBlock.tsx:32–38](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/MarkdownCodeBlock.tsx#L32-L38), lines 277–370, and `apps/web/src/lib/incrementalHighlighting.ts:57–107`.

**Commander gap:** [hub-web/src/rich-code.ts:4–19](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/rich-code.ts#L4-L19) dispatches only diagrams/diffs; regular fences remain monochrome. **Benefit:** distinguish keywords, values and comments while scanning technical answers. **M · Adapt · rank 2.** Lazy-load a small language set, cap input/cache size for phones, and preserve visible plain text while loading. Do not copy T3’s 50 MiB budget or its invisible Suspense placeholder (`ChatMarkdown.tsx:1566–1571`) without measuring our own constraints. Theme syntax separately from success/error colors; render only trusted highlighter output or safe token nodes.

### 03 · Make tables readable and exportable

**T3 Code does:** a real table inside a width-limited scroll area, with Markdown/CSV copy. Top-aligned cells and word-boundary wrapping keep most tables in the lane. See [apps/web/src/components/chat/MarkdownTable.tsx:16–69](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/MarkdownTable.tsx#L16-L69) and `apps/web/src/index.css:2197–2240`.

**Commander gap:** [hub-web/src/thread-model.ts:356–391](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/thread-model.ts#L356-L391) splits rows on every pipe; `conversation-view.ts:1847–1867` puts cell strings into spans with table roles, so cell Markdown is literal. Existing `cellTone` already highlights pass/warn/fail. **Benefit:** evidence, comparisons and file lists can be read and reused without repair. **M · Adapt · rank 3.** Use one fence-aware block parser, escaped-pipe handling, safe inline cell rendering and native table semantics. Retain useful status tones. Verify fenced pipe text remains code and CSV properly quotes commas, quotes and line breaks.

### 04 · Let tool details look like the operation

**T3 Code does:** read output, structured tool payloads, commands, viewed images and inspector details take different paths. Icons and meaningful labels support a compact work-log row; expanding reveals the appropriate body. See [apps/web/src/components/chat/TimelineWorkEntryRow.tsx:294–348](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/TimelineWorkEntryRow.tsx#L294-L348), lines 392–408 and 524–555.

**Commander gap:** [hub-web/src/work-rows-view.ts:60–106](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/work-rows-view.ts#L60-L106) already gives tools lazy, focusable details and explicit failure text, but every ready detail is one plain `pre`. `work-events.ts:9–34` has kind/label/error/detail, without typed command, result or file fields. **Benefit:** distinguish “read three files,” “ran a command” and “changed a file” before opening raw data. **L overall / M UI · Adapt · rank 4.** Add optional versioned typed payloads; keep unknown tools and legacy daemons readable. Do not recover authority, success or a diff by guessing from prose.

<figure class="tool-figure">
<p class="eyebrow">Proposed disclosure · independently authored schematic</p>
<div class="tool-sketch"><strong>Read · 3 files</strong><span>Completed</span><p>▸ Source paths · open for exact output</p></div>
<figcaption>A stable summary carries operation, scope and lifecycle; detail stays on demand. Proposed extension of Commander’s current disclosure, informed by technique 04. No backend behavior is implied. Source inspection: October 28, 2026.</figcaption>
</figure>

### 05 · Give published thinking a readable preview

**T3 Code does:** collapsed reasoning uses a Markdown preview and “Thinking” versus “Thought”; expanded reasoning renders Markdown in a bounded work-log detail, following live output only at its live edge. See [apps/web/src/components/chat/TimelineWorkEntryRow.tsx:118–138](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/TimelineWorkEntryRow.tsx#L118-L138), lines 417–428, and `chat/WorkLog.tsx:128–164`.

**Commander gap:** [hub-web/src/work-rows-view.ts:67–106](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/work-rows-view.ts#L67-L106) displays `rowText` and plain detail for thinking too; `work-events.ts:267–270` prefixes the label with “Thinking.” Existing collapsible groups and preserved focus are strengths. **Benefit:** skim the harness’s published explanation without confusing it with the final answer. **M · Adapt · rank 5.** Render only intentionally available summaries/details through the safe Markdown subset; preserve raw fallback and open state. The current event schema does not establish a reasoning start/end lifecycle, so use honest neutral labels until it does. Do not expose or synthesize private reasoning.

### 06 · Add semantic prose treatments rather than arbitrary emphasis

**T3 Code does:** real heading levels and GitHub-style note/warning/caution callouts with a label, icon and rule. See [apps/web/src/components/ChatMarkdown.tsx:929–987](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/ChatMarkdown.tsx#L929-L987); hierarchy spacing is in `apps/web/src/index.css:1888–1919`.

**Commander gap:** [hub-web/src/markdown-renderer.ts:278–294](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/markdown-renderer.ts#L278-L294) reduces all headings to one bold paragraph style; its block-start grammar at lines 14–33 has no quote/callout branch. **Benefit:** separate a caveat, evidence and the answer in long technical replies. **M · Adapt.** Introduce a modest in-message hierarchy and labeled informational callouts. Reserve Commander’s existing warm ask and critical blocker cards for actionable state; model-authored WARNING text must not become a system alert. Keep arbitrary HTML literal.

### 07 · Show the actual live phase, and stop animation when work waits

**T3 Code does:** working rows distinguish worktree setup, compaction and elapsed working time; the thinking animation is suppressed while an approval waits. See [apps/web/src/components/chat/TimelineWorkGroupRows.tsx:277–308](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/TimelineWorkGroupRows.tsx#L277-L308) and lines 360–418. This is semantic progress, not a made-up percent complete.

**Commander gap:** [hub-web/src/conversation-view.ts:1305–1312](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/conversation-view.ts#L1305-L1312) paints dots and “working” from a boolean. `work-events.ts:9–34` carries event times and errors, but no operation lifecycle or start/done pair. **Benefit:** tell active execution from waiting or setup without opening raw output. **L · Adapt after schema work.** Require authoritative phase/start/end events and stale/disconnected states. Do not infer elapsed work time from the oldest retained event. Existing delivery/connection warnings retain precedence; animation never counts as proof of activity.

### 08 · Preserve stable streamed content; optimize only when it is a real input

**T3 Code does:** completed fences followed by blank lines can form a cached Markdown prefix; definitions and CR/BOM cases fall back to a full parse. A separate stream tokenizer retains completed code lines. Live text subscriptions are below the main chat shell. See [apps/web/src/markdown-incremental.ts:33–95](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/markdown-incremental.ts#L33-L95), `lib/incrementalHighlighting.ts:75–104`, and `components/chat/ThreadTimeline.tsx:14–56`.

**Commander comparison:** [hub-web/src/conversation-view.ts:1360–1417](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/conversation-view.ts#L1360-L1417) already reuses unchanged bubbles/media; lines 581–623 protect follow intent, and lines 1295–1302 anchor opened work. `markdown-renderer.ts:255–296` parses a whole message, but the examined work-detail contract is on-demand text, not a token-delta protocol. **Benefit if streaming is added:** avoid jumping selection and repeated parsing. **L · Skip for the first epic.** Preserve existing reconciliation now. Benchmark long replies and define a delta contract before introducing incremental parser complexity. No performance result was measured here.

### 09 · Attach a changed-files summary to the turn, then open review on demand

**T3 Code does:** assistant metadata can render a changed-files card tied to a run, with persisted expansion and an “open diff” callback. Its Pierre adapter handles split-wrap column sizing, selected lines and sticky file headers. See [apps/web/src/components/chat/TimelineAssistantRows.tsx:288–370](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/TimelineAssistantRows.tsx#L288-L370) and `components/diffs/StyledDiffCodeView.tsx:15–21,80–102`.

**Commander gap:** [hub-web/src/rich-diff.ts:1–37](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/rich-diff.ts#L1-L37) already renders unified hunks, old/new line numbers, add/remove tones and disclosure after 24 lines. It starts with fenced text, not a run’s structured changed-file summary. **Benefit:** find what changed before reading a whole patch. **L · Adapt later.** Start with a provenance-bound file summary and existing unified viewer. A full split review surface, comments and a new diff library are separate scope; do not import an editor just to color a patch.

### 10 · Extend the image inspection workflow, not the thumbnail count

**T3 Code does:** an expanded image dialog supports zoom/pan, arrow-key gallery navigation, named images and explicit load failure. HEIC/TIFF can lazy-load a decoder with cancellation and URL cleanup. See [apps/web/src/components/chat/ExpandedImageDialog.tsx:109–139](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/ExpandedImageDialog.tsx#L109-L139), lines 200–232, and `components/media/AttachmentImage.tsx:9–62`.

**Commander comparison:** [hub-web/src/rich-attachment.ts:24–51](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/rich-attachment.ts#L24-L51) already reserves a viewport and offers an open-image link; `rich-media.ts:90–119` supports lazy images, audio, video and PDF dispatch. No HEIC/TIFF decode or multi-image navigation appears in these modules. **Benefit:** compare attached screenshots and inspect detail without losing the conversation. **M · Adapt if image-review use warrants it.** First inspect the existing open-image destination before adding a second viewer. Extra codecs are optional and need separate license/size review; preserve artifact authorization, retry states and node reuse.

### 11 · Keep diagram source and preview as two views of one object

**T3 Code does:** a Mermaid block toggles code/diagram and expands the diagram; it waits for a completed fence before previewing and keeps a render-error fallback. See [apps/web/src/components/chat/MarkdownMermaidCodeBlock.tsx:26–81](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/MarkdownMermaidCodeBlock.tsx#L26-L81).

**Commander comparison:** [hub-web/src/rich-diagram.ts:24–47](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/rich-diagram.ts#L24-L47) already builds a diagram and source disclosure with input/output limits and sanitized SVG; `rich-code.ts:4–19` preserves source on failure. **Benefit:** consistent Copy/source/expand affordances across technical objects. **S · Adapt the frame only.** Reuse the current renderer and its restrictions. The gap is interaction consistency, not missing diagrams. Defer completion-aware preview until Commander actually supports streaming fences.

### 12 · Use density and motion to distinguish reading from activity

**T3 Code does:** compact work-log rows have a fixed icon gutter, quiet secondary labels and grouped spacing; headings/prose have a separate rhythm. Live shine is gated by reduced-motion/forced-color preferences and visible-animation state. See [apps/web/src/components/chat/WorkLog.tsx:53–91](https://github.com/pingdotgg/t3code/blob/5211acfe95a0c6d82a60e2524b5e46b47fe67a3e/apps/web/src/components/chat/WorkLog.tsx#L53-L91) and `apps/web/src/index.css:564–576,638–653`.

**Commander comparison:** [hub-web/src/thread-model.ts:1–11](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/src/thread-model.ts#L1-L11) already groups turns and folds statuses. `work-rows-view.ts:40–55,116–158` already retains task chips in collapsed work. [hub-web/DESIGN.md:266–275](https://github.com/Richards-LLC/cassy/blob/25624dd2f2d011c515ddb4314b2541032e2faec2/hub-web/DESIGN.md#L266-L275) defines semantic colors and accessibility constraints. **Benefit:** more information per glance without making every event an alert. **S · Adapt.** Add a small consistent icon vocabulary and readable metadata; preserve phone tap targets, text labels and semantic status colors. Skip T3’s exact palette, blanket truncation and shimmer as a default. Vibrancy comes from meaningful syntax, status and hierarchy within Glass.

## Counter-evidence: Commander already has the foundations

The strongest case against a large visual rewrite is the existing implementation: grouped turns, folded status runs, lazy details, task chips, anchored expansion, unified diffs, sanitized diagrams and stable media nodes are already present in the cited Commander paths. A replacement renderer risks losing focus, offline behavior and reading position to gain a new coat of paint. T3’s React/Remark/Shiki/Pierre architecture is substantially different from Commander’s explicit DOM nodes; the source study does not establish that switching stacks would help.

T3 also demonstrates tradeoffs we should not inherit unquestioned: invisible loading placeholders, substantial cache budgets, truncated work labels and extra decoding/viewer dependencies. Its richer work UI consumes structured run/tool data Commander’s current event interface does not expose. Some visible improvements therefore require protocol work; CSS alone cannot make reliable lifecycle or provenance.

## Confidence, gaps and what would change the ranking

**Verified by source:** pinned files implement the described branches, and the thread render path references the examined components. **Not verified:** upstream runtime appearance, accessibility, frame timing, memory use, or whether each optional feature appears for every provider. Upstream was deliberately never run. The report’s diagrams communicate structure and proposed Commander treatment; they are not captures of either live product.

The table-parser gap and ordinary-code gap are direct observations. Tool and streaming recommendations have more uncertainty because server/provider event production was outside this UI-focused scope. A follow-up must trace source schemas before promising lifecycle, structured diffs or streamed reasoning. The image study is limited to inline media and T3’s expanded dialog; Commander’s existing open-image destination was not audited.

Ranking would change if operator usage shows diffs or screenshot comparison dominate ordinary technical replies, if published reasoning summaries are seldom available, or if a long-message benchmark exposes a current rendering bottleneck. Validate with representative saved transcripts, two light/dark phone-width reading sessions and measured render/selection behavior before expanding scope. Do not claim latency or comprehension gains until measured.

## Licensing and implementation boundary

T3 Code’s pinned root LICENSE is MIT with copyright 2026 T3 Tools Inc.; its notice condition applies to copies or substantial portions. This report borrows interaction techniques and uses independent prose/diagrams, with no copied implementation, assets or icon set. A future dependency or code import needs its own license/notice review; the root license is not evidence that every third-party package is MIT. Keep Commander’s code, tokens, SVG restrictions, artifact access rules and message authority model.

## Provenance and reproducibility

Study window: October 28, 2026, approximately 10:48–11:05 AM ET. The commit date is October 28, 2026, **9:02:48 AM ET** (Git records `2026-10-28T06:02:48-07:00`). “Latest” means the default-branch snapshot fetched during this window, not a promise about later commits.

Commands used to establish the snapshot and inspect source:

```text
exa-search --contents https://github.com/pingdotgg/t3code
git ls-remote --symref https://github.com/pingdotgg/t3code.git HEAD
git clone --depth=1 --single-branch https://github.com/pingdotgg/t3code.git /mnt/rewind/cas/scratch/cas-b4e8-t3code
git -C /mnt/rewind/cas/scratch/cas-b4e8-t3code show -s --format='%H%n%cI%n%s'
rg --files apps/web/src/components
rg -n 'stream|remark|rehype|table|highlight|shiki|cache|memo|code|mermaid|alert' apps/web/src/components/ChatMarkdown.tsx
nl -ba <file> | sed -n '<ranges>p'
```

The committed search manifest names every upstream/local source file read, distinguishing targeted excerpts from filename-only search results. Local source citations use the Commander baseline SHA; upstream links use the complete fetched SHA. Follow-up estimates are author judgments based on existing seams and missing data contracts. No runtime metrics or user-study data were collected.

Markdown source: [report.md](https://github.com/Richards-LLC/cassy/blob/factory/rapid-hound-89/docs/analysis/2026-10-28-t3code-chat/report.md). [Search manifest](https://github.com/Richards-LLC/cassy/blob/factory/rapid-hound-89/docs/analysis/2026-10-28-t3code-chat/search-manifest.md). Report date: October 28, 2026. Prepared for Commander follow-up planning.
