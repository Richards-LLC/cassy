# Violet integration JSON

| Field | Contract |
| --- | --- |
| First two lines | Human mode retains the existing integration verdict and hub row; JSON mode starts the VioletReport object. |
| Scannable | Scripts receive exactly one JSON document, with the existing probe and remedy fields. |
| Readable | Human summaries retain their existing wording; refusal diagnostics still explain the failed credential verification. |
| Machine output | `cas integrate violet --json` emits one VioletReport on stdout, including when a completed probe rejects authentication; failures retain a nonzero exit and diagnostic on stderr. Errors before report creation emit no report. |
| Omitted | JSON stdout excludes the human IntegrationOutcome summary, including the unauthorized-probe summary; credential values remain absent. |

Human layout, colors and path rendering are unchanged. Existing terminal layout
findings remain tracked separately; this change repairs the JSON stream only.

## Verification

The integration_cli regressions drive both global --json positions, ordinary
human output, and a local hub returning HTTP 401. Compilation and real-build
terminal captures belong to supervisor assembly; no worker cargo run is authorized.
