# Task create: inherited release binding

| Field | Contract |
| --- | --- |
| First two lines | The existing Created task verdict is followed immediately by the inherited epic and active task provenance, so a worker can see which release owns the follow-up. |
| Scannable | Labelled repository and branch rows show the exact persisted WorkTarget, including an active task's branch that differs from its epic's live lane. |
| Readable | The source task ID explains the default; an ambiguous active scope refuses creation and asks for an explicit epic or target binding. |
| Machine output | This is MCP task action=create: the existing CallToolResult content array contains a text item; there is no CLI --json mode or new structured field. The persisted task and parent edge remain the machine authority. |
| Omitted | Lease internals and competing candidates stay out of a successful receipt; explicit bindings use the existing receipt and resolver. |

The added receipt uses ASCII words, no colour or terminal control codes, and
prints complete binding values. The captured regression fixture is checked
against the real handler response; terminal captures replay those exact bytes.
Existing optional create warnings and caller-supplied titles are outside this
receipt change. No field is truncated, so long user-supplied values remain
copyable in MCP clients.

## Critique

terminal-qa: PASS task-create-inheritance · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Line one confirms creation; line two names the inherited epic and active task. |
| Fit | 4 | Four rows fit at 80 and 120 columns, including the original v57 release branch. |
| Craft | 4 | The two binding rows share indentation and retain complete copyable values. |
| Theme safety | 5 | ASCII monochrome captures pass all four palettes, pipe, NO_COLOR and C locale. |
| Machine contract | 4 | The existing MCP text envelope is preserved; the real handler test pins this normalized receipt. |

Scored by noble-pelican-42 on 2026-10-22. Dark and light 80-column HTML
captures were inspected using a disposable headless Chrome screenshot because
Playwright MCP's Chrome sandbox could not launch. This is static fixture layout
evidence; execution of the create path is proven separately by the Rust test.
The gate's report and full captures are retained in the task artifact directory.
