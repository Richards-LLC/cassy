# Unpublished peer epic diagnostic

| Field | Contract |
| --- | --- |
| First two lines | Readiness names the source host: restore a missing lane here, or publish from its peer host. |
| Scannable | One readiness verdict, one recovery/publication instruction, one raw shell command on its own line. |
| Readable | Missing refs drive recovery on the source device; legacy host labels are sanitized, and branch controls appear as visible escapes. |
| Machine output | Existing spawn failure transport is unchanged; this is its error string, with no new JSON document or fields. |
| Omitted | Creation history stays in task deliverables; no source-host path or automatic branch recreation is printed. |

The branch and originating host travel together in sync-safe task deliverables.
A mismatched branch or legacy task uses a neutral missing-lane diagnostic.
A previously observed remote ref still refuses automatic resurrection. Existing published adoption never invents an origin host.

Device identity takes precedence over hostname when recorded. This distinguishes
peers sharing a default hostname and still recognizes a renamed source device.
Legacy records retain hostname comparison. A control-bearing branch gets a
visible escaped label and manual rename/publish advice, with no command.
Other remedies use one shared argument formatter: the ASCII allowlist is bare,
all other printable arguments use ordinary single quotes. Multiple command
steps validate together, so one unsafe ref suppresses the whole remedy block.
Git advice in close gates, merge and spawn failures, hook guards, assignment,
stash recovery, doctor and director prompts shares that formatter. Mixed
Git/gh/cas steps use its same atomic argv primitive. Git execution still receives
the original refs; escaped labels are for display only. Synced provenance adds
an optional `epic_branch_origin.device_id`; older records remain readable.

## Critique

terminal-qa: PASS epic-peer-publication-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Readiness and source host are line one, recovery/publication line two, a raw command line three. |
| Fit | 4 | Three plain lines; the fixture fits 80 and 120 columns. |
| Craft | 4 | One cause and one shell-safe command without markup; no source path or guessed restore tip. |
| Theme safety | 5 | Monochrome ASCII passes four palettes, NO_COLOR, pipe and C locale. |
| Machine contract | 4 | Existing spawn-error transport remains intact, with no extra stdout document. |

Scored by sharp-cobra-83 on 2026-10-22. This is fixture layout evidence;
the actual two-clone spawn regression pins the normalized diagnostic and
executes the whole peer command line, including a shell-metacharacter branch, without command substitution. Arbitrarily long branch names remain complete
in the copyable command and can exceed terminal width.

terminal-qa: PASS epic-source-recovery-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

The source-host capture comes from the real focus receipt and preserves a clean
reflog command line before the separate focus-status footer. Existing published
adoption leaves origin provenance unknown instead of naming the adopter.

terminal-qa: PASS epic-control-ref-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS epic-brace-ref-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Control refusal names the escaped ref on line one and manual action on line two; printable refs retain one command on line three. |
| Fit | 4 | All four fixtures pass 80/120-column captures; control text occupies two lines with a maximum width of 64. |
| Craft | 4 | Brace/history arguments use ordinary shell single quotes; control-bearing refs never produce a command. |
| Theme safety | 5 | Monochrome text is readable on all four palettes, NO_COLOR, pipe and C locale. |
| Machine contract | 4 | Error strings retain their existing transport; the new optional device field has an object/string JSON roundtrip regression. |

Scored by wise-sparrow-74 on 2026-10-22. All four fixture gates were rerun;
these are layout captures, not a compiled CLI walk. The real peer-preparation
test pins the new brace and control captures, executes printable remedies in
interactive Bash, and reads back origin refs. The HTML captures were inspected
on dark and light backgrounds. Playwright MCP could not launch because the host
Chrome sandbox is misconfigured; disposable headless Chrome rendered the static
capture HTML instead. No application server was started.

## v58 follow-ups

| Field | Contract |
| --- | --- |
| First two lines | Keep the original stale-hook, spawn-create or merge-failure verdict and its cause before advice. |
| Scannable | Main-checkout remedies remain the two existing alternatives; a refused merge remedy retains the original merge class and conflicting paths. |
| Readable | Folder arguments preserve their exact shell argv; branch labels expose C1, bidi and default-ignorable characters as Unicode escapes. |
| Machine output | Existing io::Error kind and MCP wire error code remain unchanged; only diagnostic and remedy text changes. |
| Omitted | No automatic checkout, normalization, branch rename or merge is performed; unavailable remedies get a visible refusal. |

Unicode policy: treat Unicode17 Default_Ignorable_Code_Point values plus control
characters as unsafe for copyable shell advice, and escape them in displayed
labels. Ordinary visible Unicode remains available through shell quoting.
The property includes bidi controls, zero-width joiners/spaces, fillers and
variation selectors ([Unicode UAX31](https://www.unicode.org/reports/tr31/#Layout_and_Format_Control_Characters),
[pinned data](https://www.unicode.org/Public/17.0.0/ucd/DerivedCoreProperties.txt)).
This is a display policy for recovery advice; Git still receives the original
refs through argv. It does not claim to detect every confusable identifier.

### v58 critique

terminal-qa: PASS epic-invisible-ref-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS epic-control-ref-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS epic-brace-ref-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Invisible-ref verdict exposes the original ref on line one; line two names manual recovery and the unavailable command. |
| Fit | 4 | The new two-line capture is 78/63 columns and passes 80/120-column gates; existing C1 and printable-ref captures still pass. |
| Craft | 4 | RLO is a visible Unicode escape; the real peer-readiness test pins this capture and suppresses executable advice for RLO and zero-width space. |
| Theme safety | 5 | Default-foreground ASCII is readable on dark, light and both Solarized captures, with clean pipe, NO_COLOR and C-locale runs. |
| Machine contract | 4 | The existing error transports and MCP code remain unchanged; safe merge diagnostics retain their commands and refused advice retains the original cause. |

Scored by wise-sparrow-74 on 2026-10-22. These gates prove fixture layout,
not an actual CLI walk. The 80-column HTML was rendered with disposable headless
Chrome and inspected on all four palettes. Playwright MCP launch failed because
the host Chrome SUID sandbox is misconfigured; no application server was used.
The Rust regressions separately exercise real Git branch materialization,
two-clone peer preparation and both printed stale-hook remedies in interactive
Bash. Existing full stale-hook and merge diagnostics retain their prose layout;
unbounded paths/refs can still exceed terminal width.
