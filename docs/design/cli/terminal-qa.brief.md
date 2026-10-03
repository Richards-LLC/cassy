# Terminal capture integrity

| Field | Decision |
| --- | --- |
| First two lines | A PASS receipt means the requested command completed and produced visible output in every selected capture; unavailable capture exits 2. |
| Scannable | Keep the receipt's verdict, run count and finding counts stable; empty captures appear as unallowlistable failures. |
| Readable | Preserve the runner diagnostic for unavailable captures and direct empty-output findings toward a command that prints the intended surface. |
| Machine output | report.json retains its verdict/runs/findings/counts shape; capture_bytes and stdout/stderr file paths are additive fields, and JSON command stdout remains separate. |
| Omitted | PTY completion markers stay outside the evaluated byte stream; expected command failure codes remain evidence rather than rendering defects. |

Human pipe captures concatenate stdout then stderr and retain each original stream as a
sidecar. PTY captures retain their actual terminal ordering. JSON stderr receives pipe checks
without being appended to the JSON document. BSD/macOS uses script's positional command
form; util-linux keeps its -c form. Both run a POSIX shell, set the requested geometry and
require a private command-completion marker. An unavailable rerun removes older report
receipts from the same output directory before attempting capture.

The production runner's planted-defect and capture-integrity tests establish the receipt
contract. This changes capture validation, not the existing receipt layout.
