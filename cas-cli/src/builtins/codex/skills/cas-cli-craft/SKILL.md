---
name: cas-cli-craft
description: Use when designing, rewriting, or critiquing anything a CLI or TUI prints for a person — a status screen, doctor-style report, table, progress line, error, warning, receipt, or the human side of a --json command — before the first render and before it merges. Owns the concept brief, output hierarchy, the semantic colour set that survives light and dark terminals, width and Unicode fallbacks, the --json contract, and the scored critique that requires a terminal-qa PASS receipt; cas-ui-craft owns HTML surfaces.
managed_by: cas
---

# CLI craft

Terminal output is finished when a reader gets the verdict from the first two lines, can act
from one copyable command, and nothing wraps, clips, or disappears on a light or dark screen.

## Steps

1. **Write the concept brief before touching the renderer.** Create
   `<command>.brief.md` beside the source (or under `docs/design/cli/`) using
   [references/concept-brief.md](references/concept-brief.md). Done when all five fields hold a
   sentence specific to this command, and the *machine output* field names the `--json` shape.
2. **Order the output by the hierarchy.** Verdict line, then grouped rows, then remedies as one
   command each, then receipts and timing last; details only under `--verbose`. Done when the
   first two lines alone answer "is it fine, and what do I do". The full rules are in
   [references/output-contract.md](references/output-contract.md).
3. **Colour only the marks, from the semantic set.** Success, warning, error, info, muted —
   nothing else carries meaning, and every meaning is also carried by a glyph or word; the
   verdict word is bold in the terminal's default foreground, as is all body text. Done when
   `NO_COLOR=1` output reads identically and every coloured run measures ≥ 4.5:1 for text and
   ≥ 3:1 for a mark on the dark, light, and both Solarized palettes.
4. **Fit 80 columns; use 120 for breathing room, never for content.** Tables right-align
   numbers, state units once in the header, and never wrap a row; long values truncate with `…`
   and the output names the escape (`--full`, or `--verbose` where the command already has
   one). Done when the 80-column capture has no line over 80 cells and no token split across
   lines.
5. **Shape every error and warning as name → cause → one remedy.** Three parts, one screen line
   each where possible, never a paragraph; the remedy is a command the reader can paste. Repeated
   findings collapse to one row with a count. Done when no warning exceeds three lines.
6. **Separate TTY from pipe and Unicode from ASCII.** Spinners, redraws, and cursor codes only
   when stdout is a TTY; `--json` prints one document and nothing else on stdout; box drawing and
   glyphs fall back to ASCII when the locale is not UTF-8. Done when the piped, `NO_COLOR`, and
   `LC_ALL=C` runs each read cleanly.
7. **Run the gate and critique.** `node scripts/terminal-qa.mjs --label <command> -- <command>`
   (add `--escape-flag --verbose` or `--json-flag --json` as the command warrants) captures 80
   and 120 columns on four palettes plus the piped, `NO_COLOR`, and C-locale runs, and writes
   `report.md` whose first line is the receipt. Score the output with
   [references/critique-rubric.md](references/critique-rubric.md), paste the receipt line and
   the score table into the brief under `## Critique`. Done when the receipt says `PASS`, the
   mechanical zeros are absent, and hierarchy, fit, and craft are each ≥ 4.
8. **Commit brief, renderer, captures, and snapshot tests together.** Name the snapshot rows in
   the commit message. Done when one commit carries all of them.

## The first two lines

Line one is the verdict: one glyph, one word a reader can repeat ("healthy", "2 warnings",
"failed"), and the one number that justifies it. Line two is either the single remedy or the
first group heading. A reader who stops here has not been misled; everything below is evidence.

## Exemplars

Each carries its brief, the render at 80 columns, annotations on every decision, and its
rubric scores.

- [references/exemplars/status-screen.md](references/exemplars/status-screen.md) — a factory
  session status: verdict, three grouped rows, one next action.
- [references/exemplars/doctor-report.md](references/exemplars/doctor-report.md) — a checks
  report: collapsed healthy groups, findings as name → cause → remedy, receipt last.
- [references/exemplars/long-running.md](references/exemplars/long-running.md) — a multi-phase
  command: TTY progress that redraws, piped output that appends, one receipt either way.
- [references/exemplars/before-after.md](references/exemplars/before-after.md) — the same
  doctor output the old way and this way, each scored on the rubric.

## Scope boundaries

`cas-ui-craft` owns HTML and application screens; `cas-html-reports` and `cas-dataviz` own
reports and figures. This skill owns what a terminal shows and what a pipe receives. Renderer
mechanics (a `Formatter`, a theme palette, a `Table` component) belong to the project; this
skill decides what they should produce and whether the result is good enough to ship.
