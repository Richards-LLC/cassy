# Violet hub probe

| Field | Contract |
| --- | --- |
| First two lines | Integrate retains its configured/stale verdict; doctor retains its row severity, with drift measured against the server this project dispatches to. |
| Scannable | The integrate hub row and doctor probe message identify the effective project URL, falling back to the machine registration. |
| Readable | Existing drift and connectivity remedies follow the selected hub; missing credential references name variables without values. |
| Machine output | VioletReport JSON identifies the effective hub in url and references in probe_env_states; doctor JSON preserves message and severity fields. Existing integrate dispatch appends human text after JSON (follow-up cas-4362), so this task does not claim valid integrate JSON stdout. |
| Omitted | Bearer tokens and header values stay out of output; the proxy receives the complete configured transport, auth and header references. |

This change adds endpoint evidence to existing output without changing its layout or colors.

## Critique

Doctor: terminal-qa PASS, 12 runs, 0 fail, 0 warn, 0 allowed.
Integrate: terminal-qa PASS, 11 runs, 0 fail, 0 warn, 37 baseline findings allowed.
Native receipts and captures: `/Users/pippenz/.cas/artifacts/cas-src-ec436edb9fa83e1c1bea3ce9f62e009b4ec7a1295a20c57e7bc77fc869972300/cas-8121/terminal-qa/doctor-reviewed/report.json`
and `/Users/pippenz/.cas/artifacts/cas-src-ec436edb9fa83e1c1bea3ce9f62e009b4ec7a1295a20c57e7bc77fc869972300/cas-8121/terminal-qa/integrate-reviewed/report.json`.

| Dimension | Doctor | Integrate endpoint rows | Evidence |
| --- | ---: | ---: | --- |
| Hierarchy | 4 | 4 | Doctor's Integrations finding names the project hub before its remedy; integrate retains the verdict and hub row first. |
| Fit | 4 | 4 | The new doctor finding fits 80 columns; both text and doctor JSON retain the actual endpoint and missing variable. |
| Craft | 4 | 4 | Endpoint and reference rows use existing grammar without new columns or colors. |
| Theme safety | 5 | 5 | Native four-palette, NO_COLOR and C-locale captures pass for changed rows. |
| Machine contract | 5 | — | Doctor emits one JSON document. Integrate's inherited mixed JSON/text output remains cas-4362. |

Scored on 2026-10-04 for the endpoint-selection change. Whole-command integrate
craft remains below the floor: inherited path/remedy rows overflow and its
project separator is Unicode under the C locale. The precise allowlist, raw
failures and follow-up cas-46b0 retain that evidence; no endpoint/reference row
is exempted. The installed older terminal tool's empty macOS PTY receipts are
discarded; these receipts use the corrected builtin from the current epic.
