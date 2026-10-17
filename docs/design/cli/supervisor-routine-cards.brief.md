# Supervisor pane: routine cards

Rows the factory daemon types into a supervisor pane that runs over the PTY (Codex, or Claude
without Agent Teams). Renderer: `compact_routine_supervisor_body` in
`cas-cli/src/ui/factory/daemon/runtime/delivery.rs`, called where `queue_and_events.rs` builds
the typed payload.

| Field | Contract |
| --- | --- |
| First two lines | The `[cas #N …]` header names the sender class and id, then one glyph line says what happened: `◦ cas-f9b7 started by tender-phoenix-59` or `⇄ worker-a → worker-b · <summary>`. |
| Scannable | Routine rows are three lines at most, under 76 columns, with a glyph per class (`◦` for a progress relay, `⇄` for a peer copy). Operator turns stay whole, as do peer-supervisor mail, blockers, merge requests and verification events. |
| Readable | One gist line (the task title, or the start of the peer message, cut at a word with `…`) and a last line naming the escape: `no action · full text: inbox_poll notification_id=N`. |
| Machine output | The queue row is unchanged. `inbox_poll notification_id=N` returns the full body whatever its receipt says, and ids, ack, `in_reply_to` and wake authority are unchanged. |
| Omitted | The `<task-lifecycle>` XML, the occurrence stamp and the session line of progress relays, plus the body of a peer copy past its first line. All of it is one `inbox_poll` away. |

Routine means one of two classes, each identified by server-stamped facts:

- a non-waking lifecycle relay (task started, ready again, closed): source `lifecycle:` plus
  daemon origin;
- the supervisor's copy of worker-to-worker mail: registered-agent origin, plus a lead built by
  CAS that names the same sender.

A row that is urgent, typed (`kind`), operator-stamped, outside the ordinary lane or
unparseable is typed whole. Progress relays are now labelled `lifecycle-relay`; before this
change they were labelled `agent-authored`.

## What CAS does not own

The `• Called cs.coordination({…})` cells are Codex's own MCP tool-call rendering. Codex 0.163.0's
`[tui]` table (`animations`, `show_tooltips`, `raw_output_mode`, `status_line`, `keymap`, …) has
no setting that collapses tool-call arguments by default. The supported controls are
interactive:

- `tui.keymap.global.focus_activity` ("Focus transcript activity groups to expand or collapse
  their details").
- `toggle_raw_output`, which gives selection-friendly text and does not collapse anything.

This change leaves `~/.codex/config.toml` alone, so tool-call noise remains until Codex adds a
default.

## Critique

terminal-qa unavailable: the surface is text typed into a live Codex TUI, not a command's stdout.
It was checked by hand on real Codex 0.163.0 panes at 80×60 and 120×60, with the same payloads
and an operator turn for contrast. Captures are in [captures/before/supervisor-routine-cards](captures/before/supervisor-routine-cards)
and [captures/after/supervisor-routine-cards](captures/after/supervisor-routine-cards). The ANSI,
HTML and PNG sets for both widths are in the task artifact directory `cas-8cbe/captures/`.

| Pane | Lifecycle relay | Peer copy | Operator turn | Widest line |
| --- | ---: | ---: | ---: | ---: |
| before, 80 | 8 lines | 7 lines | 6 lines | 80 |
| after, 80 | 4 lines | 4 lines | 6 lines | 80 |
| before, 120 | 7 lines | 5 lines | 4 lines | 119 |
| after, 120 | 4 lines | 4 lines | 4 lines | 119 |

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Line one after the header says who did what (`◦ cas-f9b7 started by tender-phoenix-59`); the escape command is always the last line. |
| Fit | 5 | Both routine classes shrink to a card while the operator turn keeps its full body and reply command at both widths. |
| Craft | 4 | No card line wraps at 80 columns; Codex keeps the two-space hanging indent; the gist cuts at a word. The `cas #` header still wraps for long operator labels (unchanged). |
| Theme safety | 4 | The payload is plain text, so no SGR is typed. The glyphs (`◦ ⇄ → · …`) rely on a UTF-8 pane, as the existing `—` in relay text already did. |
| Machine contract | 5 | The queue row, receipts and wake classes are unchanged, and the payload still starts with the literal `Message from <sender>:` prefix. |

Scored by steady-octopus-37 on 2026-10-17; the floor holds.

To review again, run `bash <artifacts>/cas-8cbe/run-pane.sh after 80 60` (or `before`, or
`120`). It replays the payloads into a scratch Codex pane with MCP disabled, saves `.txt` and
`.ansi`, then exits. Nothing records in the background.
