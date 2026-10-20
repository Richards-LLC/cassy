---
name: slack-formatting
description: Use when writing any Slack message body (a violet_post message, thread, reply or file comment, a push-wake answer, a status or investigation report) so a human can take it in at a glance instead of facing a wall of text.
---

# Format Slack messages for humans

People scan; they do not read. In NN/g eyetracking studies 79% of users scanned a new page and 16% read word by word, and a concise, scannable rewrite scored 58% higher in usability. Unbroken text produces the F-pattern: readers catch the first words of the first lines and skip the rest. Slack makes this worse. A notification or mobile preview shows only the first line, and long messages collapse behind "Show more". Your job is to make the first line carry the point and everything else easy to skip.

## The ten rules

1. **First line = the answer.** One plain sentence of at most ~100 characters: the verdict, decision or ask. It is not a title, and it carries no second clause. A reader who stops here still knows the one thing that matters.
2. **Top-level is short; detail goes in the thread.** Keep the channel message to ≤ 8 lines and ≤ ~600 characters. Put evidence, breakdowns, per-item numbers and caveats in threaded replies, one reply per topic. Use `kind: "thread"` with `replies[]`.
3. **One idea per line, one line per bullet.** A bullet should fit on one rendered line (≤ ~90 characters). If it needs a second clause, split it into two bullets or move it to the thread.
4. **Blank line between blocks.** Separate the headline, each section and the closing ask with an empty line. Never stack more than 5 bullets without a break or a label.
5. **Hierarchy through structure, not bold.** Bold only short labels (2–4 words) and the few numbers that matter. Never bold a whole sentence or the headline. Show sub-points as an indented `◦` under a `•`, at most two levels deep.
6. **End with the ask or the next step**, on its own line: who does what, by when. If nothing is needed, say so in three words or say nothing.
7. **Meaning before data.** Every section opens with a one-line takeaway in words ("All post types on time except 2 deleted reels"), then the figures. A list of bare numbers is a spreadsheet, not a message. Short is not the goal; easy to understand is.
8. **Numbers must reconcile on sight.** If 253 were due and 251 were on time, the reader must see where the other 2 went in the same line. Never print "0 missed" beside a gap you explain somewhere else.
9. **Never name a file the reader can't open.** Don't mention a path, filename or artifact id unless that file is attached to the same message as a downloadable file. Otherwise attach it, or describe what it shows without naming it. A link to a page people can open is fine.
10. **Times in CT, never UTC.** Write `2:08 PM CT`. ET is fine when labelled. Never write UTC, GMT, `13:08Z`, or ISO timestamps ending in `Z` or `+00:00`. Convert receipts and log times before posting.

## Rules 9 and 10 are hard rules

The operator set these two, and they are never traded for brevity or tone. They apply to prose: text inside a fenced code block and inside a Slack `<url|label>` link is exempt. `cas violet post|thread` refuses a draft that breaks them before sending, and the hub refuses it with `format_unreadable` (`unattached_file`, `utc_time`).

**Rule 9 catches** absolute or `~/` paths, relative paths with an extension, bare filenames with common extensions, and `art-xxxxxxxx` artifact ids. It passes only when the same message attaches a file of that name.

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

Slack uses a proportional font and mrkdwn has no tables, so space-padded columns never line up. Pick one:

- **≤ 5 rows:** one bullet per row, label first, then the figure: `• Reels — 143 of 145 (2 deleted)`.
- **> 5 rows or several numeric columns:** put a monospace table in a code block, padded so columns align, with a header row. Put the takeaway sentence above the block, never inside it.

```text
Type        On time  Due
Reels           143  145   2 deleted
Carousels        40   40
YouTube          48   48
```

## Slack mrkdwn (what the hub sends)

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

### Status or investigation result

```text
<verdict in one sentence>

*What happened*
• <cause 1 — one line>
• <cause 2 — one line>

*What's next*
• <action> — <owner/when, time in CT>

Details are in the thread.
```

Thread replies: one per cause, each with a `*label*` line, then ≤ 5 one-line bullets.

### Answer to a question

The direct answer first (yes / no / the value), then at most 3 supporting bullets. Nothing else.

### Release notes

Follow the project's `docs/release-notes/RUBRIC.md`; these rules govern the layout inside it.

## Before you post — self-check

Refuse to send and rewrite if any of these is true:

- A path, filename or artifact id is named that is not attached to this message (rule 9).
- A time is in UTC or GMT, or ends in `Z` or `+00:00`, rather than CT or labelled ET (rule 10).
- The first line is over ~100 characters, bold, or two sentences.
- The top-level message is over 8 lines or ~600 characters and has no thread replies.
- Any bullet or paragraph line is over ~200 characters.
- More than 5 consecutive lines with no blank line.
- A whole sentence is bold, or there is `**`, a `#` heading or a `-` bullet.
- Columns are aligned with spaces outside a code block, or a section is figures with no takeaway sentence.
- An emoji is decoration, not a status marker.
- The figures on screen do not add up without reading another section.

`cas violet post|thread` refuses a draft that breaks rule 9 or 10, and the hub refuses unreadable text with `format_unreadable`. Both print each violation's rule, line, excerpt and fix: apply the fix and post again.

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
