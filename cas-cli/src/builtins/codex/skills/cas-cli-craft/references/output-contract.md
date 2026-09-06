# Output contract

Rules consulted while rendering. Each rule appears once; the critique rubric scores against them.

## Hierarchy

1. **Verdict line** — glyph, one repeatable word, the justifying number:
   `✓ healthy · 24 checks · 1.2s` or `⚠ 2 warnings · 22 ok · 1.4s`.
2. **Grouped rows** — one line per group when healthy (`Store   ✓ database ✓ schema ✓ tasks`),
   one row per finding otherwise. Group labels share a fixed left column so the eye drops
   straight down.
3. **Remedies** — one pasteable command per finding on its own line, prefixed `→`. Two
   findings with the same remedy share one line. Never a sentence that *describes* a command.
4. **Receipts** — counts, elapsed time, artifact paths, exit reason. Last, muted, one line.
5. **Details** — timings, per-item transcripts, raw values: only under `--verbose`.

Headings are words with a fixed column, not banners. A rule (`────`) separates the verdict from
the body once; no double rules, no boxes around paragraphs.

## Colour as meaning

| Meaning | Where it appears | Glyph (UTF-8 / ASCII) | Colour |
| --- | --- | --- | --- |
| success | verdict, mark | `✓` / `[OK]` | green |
| warning | verdict, mark | `⚠` / `[WARN]` | amber |
| error | verdict, mark | `✗` / `[ERROR]` | red |
| info | mark | `ℹ` / `[INFO]` | blue |
| muted | receipts, rules, hints | none | dim or gray |
| emphasis | verdict word, group label, table header | none | bold, default foreground |

- Colour is applied to the glyph, never to a word, a whole line, or a paragraph. The verdict
  word, body text, values, and remedies use the terminal's default foreground.
- Each meaning is also carried by its glyph or word, so a monochrome reader loses nothing.
- Choose colours per detected background (OSC 11, then `COLORFGBG`, then dark). Every colour
  must measure ≥ 3:1 for a mark and ≥ 4.5:1 for text on the dark, light, and both Solarized
  palettes in the gate; ANSI yellow and green fail on light backgrounds, so amber and green are
  theme values, not ANSI 3 and 2.
- `NO_COLOR` set, or stdout not a TTY: emit no SGR at all.

## Tables and alignment

- Numbers right-aligned; text left-aligned; the unit once in the header (`Time (ms)`), never
  per cell. Thousands separators for counts ≥ 10 000; one decimal for seconds.
- Column widths come from the data at the current width; a row never wraps. When the table
  cannot fit, drop the lowest-value column first, then truncate the widest text column with
  `…` and name the escape once, muted, beneath the table (`--full for untruncated values`, or
  `--verbose` where the command already has one).
- Header once, in bold; no border unless rows exceed twenty, then a single rule under the
  header. Box drawing, when used, is the light single set and falls back to `-`/`|`/`+`.

## Width

- Design for 80 columns; verify at 80 and 120. At 120 the table gains room, the prose does not
  widen past ~100 cells, and nothing new appears.
- Wrap prose at word boundaries with a hanging indent under the value column; never break
  inside a token, path, or identifier — truncate those with `…` instead.
- Width comes from the pty (`TIOCGWINSZ`), then `COLUMNS`, then 80. Below 40 columns render as
  at 80 and let the terminal wrap.

## Progress and long-running output

- TTY: one status line that redraws in place (`\r` + erase) with phase name, count, and elapsed;
  a spinner only while nothing measurable moves. Finished phases print once and stay.
- Not a TTY: append one line per phase transition with a timestamp; no redraws, no spinner,
  no cursor movement. The receipt line is identical in both modes.
- Anything over two seconds shows progress; anything over thirty names what it is waiting on.

## Errors and warnings

Three parts, each one line where possible: **name** the thing (the check, file, or flag),
state the **cause** in the reader's words, give **one remedy** as a command. A stack trace,
an internal type name, or a paragraph is a defect. Repeated findings collapse to one row with a
count and one remedy; the reader sees the instances under `--verbose`.

```
⚠ registered project roots   14 stale /tmp roots are excluded from discovery
  → cas known-repos forget --stale
```

## `--json`

- One JSON document on stdout, nothing else — no banner, no colour, no progress. Diagnostics go
  to stderr. Exit code carries the verdict.
- Field names are stable, snake_case, and typed; counts are numbers, not strings; times are
  RFC 3339 or integer milliseconds, named for their unit (`duration_ms`).
- The human render is derived from the same data as the JSON, never the other way round.

## Unicode

- Glyphs and box drawing require a UTF-8 locale (`LANG`/`LC_ALL`/`LC_CTYPE` contains `UTF-8`);
  otherwise use the ASCII column of the table above and `-`/`|`/`+`.
- Measure display width, not bytes or chars: East Asian wide and emoji cells count 2, combining
  marks 0. Emoji in output is a defect outside a deliberate brand mark.
