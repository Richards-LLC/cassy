---
name: slack-formatting
description: Use when writing any Slack message body (a violet_post message, thread, reply or file comment, a push-wake answer, a status or investigation report) so a human can take it in at a glance instead of facing a wall of text.
---

# Format Slack messages for humans

Use `violet_post` with **blocks plus fallback text** by default for status
updates, release-note threads and decisions. Fill one of the three templates
below; the project's release rubric still owns audiences, wording, order and
reply count. The layouts follow violet_ps `apps/violet/DESIGN.md` “Slack posts”.

`blocks` contains Violet layout objects, not raw Slack Block Kit: `header{text}`,
`section{text}`, `fields{fields:[{label,value}]}`, `bullets{items,ordered?}`,
`divider`, `context{text}`, or `table{rows}`. Put `type` on each object. Use one
header first, short fields for figures, tight bullets for steps, and context
last. Tables have a header row and equal-width rows; at most one per message.
See [the contract](../violet/references/contract.md#layout-blocks-default) for
all limits (header ≤100 characters; ≤10 fields; 1–50 blocks). Readability limits
apply to all visible strings as well as the separate fallback.

Always supply meaningful `text`: the one-part notification and screen-reader
fallback. Text-only thread replies carry their own `blocks` and `text`.
File replies keep the file workflow. Do not invent `kind: "blocks"`, a
`template` parameter, or raw Slack `rich_text` objects.

## The ten rules

1. **Verdict first.** The header or fallback gives the verdict, decision or ask in at most ~100 characters. A short title is fine when the next section supplies the answer. A reader who stops here still knows the one thing that matters.
2. **Top-level is short; detail goes in the thread.** Keep the channel message to ≤ 8 lines and ≤ ~600 characters. Put evidence, breakdowns, per-item numbers and caveats in threaded replies, one reply per topic. Use `kind: "thread"` with `replies[]`.
3. **One idea per line, one line per bullet.** A bullet should fit on one rendered line (≤ ~90 characters). If it needs a second clause, split it into two bullets or move it to the thread.
4. **Keep lists tight.** Use a `bullets` block without blank lines between items. In plain text, separate paragraphs and sections with a blank line, not individual bullets. Use a label when a list changes subject.
5. **Hierarchy through layout.** Use a header for the title, fields for figures and a divider only when the subject changes. In plain text a wholly bold first-line title up to 100 characters is allowed; bold body text only for short labels and key figures. Plain-text sub-points use indented `◦` under `•`, at most two levels deep.
6. **End with the ask or the next step**, on its own line: who does what, by when. If nothing is needed, say so in three words or say nothing.
7. **Meaning before data.** Every section opens with a one-line takeaway in words ("All post types on time except 2 deleted reels"), then the figures. A list of bare numbers is a spreadsheet, not a message. Short is not the goal; easy to understand is.
8. **Numbers must reconcile on sight.** If 253 were due and 251 were on time, the reader must see where the other 2 went in the same line. Never print "0 missed" beside a gap you explain somewhere else.
9. **Never name a file the reader can't open.** Don't mention a path, filename or artifact id unless that file is attached to the same message as a downloadable file. Otherwise attach it, or describe what it shows without naming it. A link to a page people can open is fine.
10. **Times in CT, never UTC.** Write `2:08 PM CT`. ET is fine when labelled. Never write UTC, GMT, `13:08Z`, or ISO timestamps ending in `Z` or `+00:00`. Convert receipts and log times before posting.

## Rules 9 and 10 are hard rules

The operator set these two, and they are never traded for brevity or tone. They apply to prose: text inside a fenced code block and inside a Slack `<url|label>` link is exempt. `cas violet post|thread` refuses a draft that breaks them before sending, and the hub refuses it with `format_unreadable` (`unattached_file`, `utc_time`).

**Rule 9 catches** `~/` paths and paths under `/home`, `/tmp`, `/mnt`, `/Users` or `/var`; relative paths (a slash plus a known extension); bare filenames with a document or data extension (`notes.md`, `report.pdf`, `data.csv`); and `art-xxxxxxxx` artifact ids. A relative path to code (`.ts`, `.js`, `.py`, `.rs`) counts only with two or more slashes or an all-lowercase first folder, so `src/App.ts` is refused while `React/three.js`, `Svelte/d3.js` and `Node.js` pass. Backticks do not exempt a name, but every URL is exempt. It passes only when the same message attaches a file of that name (compared case-insensitively).

- Don't: `Full breakdown in ~/reports/stall-summary.md`
- Don't: `See docs/postmortem.md and art-1234abcd for details`
- Do: post with `cas violet post --file postmortem.md` and the comment `postmortem.md attached: the stall began at 8:05 PM CT.`
- Do: `The breakdown is in the thread.`, with the figures written into a reply

**Rule 10 catches** `UTC`, `GMT`, `HH:MMZ`, and ISO timestamps ending in `Z` or `+00:00`. CT and ET pass.

- Don't: `Deploy finished at 2026-10-20T14:42:00Z`
- Don't: `Queue run started 22:40 UTC`
- Do: `Deploy finished at 9:42 AM CT.`
- Do: `Queue run started at 5:40 PM CT (6:40 PM ET).`

## Tables

Use a native `table` block for a small comparison: `rows` is a rectangular
array of strings and its first row is the header. Keep wide/long tables in a
reply. Use `fields` for a few label/value pairs. Never space-pad columns in
proportional text. In the plain-text fallback use one bullet per row, or a
monospace fenced table with the takeaway above it.

## Slack mrkdwn (sections and plain-text fallback)

| Want | Write | Never |
| --- | --- | --- |
| Bold | `*text*` | `**text**` |
| Italic | `_text_` | |
| Bullet | `•` at line start | `-` or `*` bullets |
| Sub-bullet | four spaces + `◦` | deeper than two levels |
| Section label | `*Label*` on its own line | `#` headings |
| Quote / callout | `>` at line start | |
| Link | `<https://url\|short label>` | bare long URLs |
| Code, IDs | backticks | unattached file paths (rule 9) |

Use emoji only as status markers that also carry a word: `✅ fixed`, `⚠️ needs you`, `🔴 blocked`. Never use emoji as decoration or as the only signal.

## Numbers

- Give one headline number per line, with its meaning: `• 97% on time (2,954 of 3,032)`.
- Round in the top-level and keep exact figures for the thread.
- Never put three or more figures in one sentence; use a short list instead.

## Shape templates

These complete example payloads target the sandbox. Replace their example
facts, channel and times with verified content before posting. Send through
`violet.violet_post`; the local CLI text flags remain a plain-text fallback.

### Status

Verdict header, key figures, done/next/waiting bullets, then context.

```json
{
  "channel": "cas-scratch", "kind": "message",
  "text": "Imports are ready; one review is pending.",
  "blocks": [
    {"type": "header", "text": "Imports are ready for review"},
    {"type": "fields", "fields": [{"label": "Ready", "value": "4 of 5"}, {"label": "Waiting", "value": "1 review"}]},
    {"type": "bullets", "items": ["*Done:* import and validation", "*Next:* review the preview", "*Waiting:* approval for the final item"]},
    {"type": "context", "text": "Preview · Tue 10 Nov, 10:30 AM ET"}
  ]
}
```

### Release-note thread

One thread per audience. Root: deploy target and audience, then one punch.
Reply: Was → Now table, or bullets with both Was and Now for each item.
Keep the rubric's exact logical reply count and ordered-thread retry rules.

```json
{
  "channel": "cas-scratch", "kind": "thread",
  "idempotency_key": "example-staging-layout-user",
  "text": "Staging · User: posts now have clear titles and compact lists.",
  "blocks": [
    {"type": "header", "text": "Staging · User"},
    {"type": "section", "text": "Posts now have clear titles, key figures and compact lists."}
  ],
  "replies": [{
    "text": "Was: plain text. Now: titles, figures and compact lists.",
    "blocks": [{"type": "table", "rows": [["Item", "Was", "Now"], ["Status posts", "A wall of text", "Title, figures and tight bullets"], ["Comparisons", "Space-padded columns", "Native tables"]]}]
  }]
}
```

### Decision

Question header, options in fields, recommendation, then owner/deadline.

```json
{
  "channel": "cas-scratch", "kind": "message",
  "text": "Decision needed by Wed 5 PM ET: ship the table layout?",
  "blocks": [
    {"type": "header", "text": "Decision: ship the table layout?"},
    {"type": "fields", "fields": [{"label": "A. Ship now", "value": "Native tables are ready"}, {"label": "B. Wait", "value": "Gather more mobile feedback"}]},
    {"type": "section", "text": "*Recommendation:* A, after the preview is approved."},
    {"type": "context", "text": "Daniel · Reply A or B by Wed 11 Nov, 5 PM ET"}
  ]
}
```

### Plain-text fallback

If the client is text-only or a confirmed older hub cannot accept blocks,
omit `blocks` and put the complete message in `text`: verdict, short labelled
sections and tight `•` lists. A release-note reply still includes Was → Now
for every item. Use CT or labelled ET. Keep the same audiences, thread shape,
idempotency key and receipts; do not replay an uncertain structured write as
plain text. Resolve its receipt first. A normal structured post keeps its
fallback to one meaningful part rather than duplicating the entire layout.

For a direct answer, the value or yes/no first, then at most three supporting
points; use a section or a short plain-text reply when a layout adds nothing.

## Before you post — self-check

The rules above are the target. The Violet hub refuses a post outright, with `format_unreadable`, when it breaks any of these nine limits. Check both fallback text and every visible block string. Fenced code and URLs retain their exemptions; thread replies are exempt from `first_line` and `long_top_level`. Layout roots have a combined 900-character visible-text ceiling.

| Hub rule | Refused when |
| --- | --- |
| `first_line` | the top-level message's first line is over 160 characters, or is wholly bold and over 100 (including its `*` markers) |
| `long_line` | any line is over 300 characters |
| `no_breathing_room` | more than 6 prose lines run together without a blank line; tight bullet items do not add to that count |
| `long_top_level` | the top-level message is over 900 characters |
| `markdown_syntax` | a line has `**`, starts with a heading (`#` to `######` then a space; `#channel` is fine), or starts with a `-` or `*` bullet |
| `fake_table` | 3 or more consecutive non-blank lines each have 2 or more gaps of 3+ spaces between words (a blank or non-matching line ends the run) |
| `bold_sentence` | a body bold span is over 80 characters; a whole-bold first-line title ≤100 characters is allowed in fallback text |
| `unattached_file` | rule 9: a path, filename or artifact id is named and no file of that name is attached |
| `utc_time` | rule 10: a time shows UTC, GMT, `HH:MMZ`, or an ISO timestamp ending in `Z` or `+00:00` |

Before you send, also check what the hub cannot: the first line is the verdict, sections lead with a takeaway, figures add up, and emoji are status markers, not decoration.

`cas violet post|thread` refuses a draft that breaks rule 9 or 10 before sending, and the hub refuses the other limits. Both print each violation's rule, line, excerpt and fix, and the fix for a file or time names the exact token. Apply the fixes and post again.

## Over-correction (also don't)

When told "too long", agents swing to a terse data dump: a bold title, a row of figures, and space-padded columns with no words. That is equally unreadable. The fix is structure plus meaning, not fewer words:

> *Snapshots since yesterday's release* ✅
> *253* due · *251* on time · *0* missed
> …
> *By post type* (on time / due)
> Instagram reels   143 / 145 (other 2 = deleted posts)
> Instagram carousels   40 / 40
> …

What's wrong: the headline states a topic, not a verdict. "0 missed" sits beside an unexplained gap of 2. The columns are space-aligned in a proportional font. The sections are raw figures with no takeaway, and the emoji are decorative.

Better:

```text
All 251 snapshots since yesterday's release were on time. The other 2 were deleted posts we skip on purpose.

• Typical capture: 36 sec late, slowest 3 min
• New posts: median pickup 5 min (31 of 36 within 15 min)
• ⚠️ 3 creators: Instagram is refusing their data, retrying hourly

Per-type numbers and today's cleanup are in the thread.
```

## Example

Wall of text (don't):

> *Missed snapshots: checked every post since 5 Oct. The scheduler is solid; the misses have three causes, one already fixed*
> *Snapshot health, posts published 5–19 Oct*
> • 3,032 snapshots came due while the post was live and in Pulse Card. 2,954 captured on time: all within 17 minutes, 99% within 2 minutes.
> • The other 78: 77 belong to 17 posts the creator deleted from Instagram, and 1 was a provider rate limit. …(12 more two-line bullets)

Scannable (do), top-level:

```text
Snapshots are healthy: 97% on time since 5 Oct. The misses have three causes, none ongoing.

*Why some were missed*
• ✅ 15 Oct backend outage, 9h — fixed that evening
• Creator deleted the post (17 posts) — expected, not our bug
• Post went public after its Instagram date (14 posts)

*Needs you*
• Decide whether deleted posts should show "deleted", not "missed"

The breakdown and the example post are in the thread.
```

Thread replies: `*Numbers*` (the 3,032 / 2,954 / pickup-time figures, one per line), `*15 Oct outage*`, `*Deleted posts*` (per-creator counts), `*Late-public posts*`.
