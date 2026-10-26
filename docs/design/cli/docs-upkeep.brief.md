# Documentation upkeep refusal contract

| Field | Contract |
| --- | --- |
| First two lines | A close refusal starts with CODEMAP DRIFT or PRODUCT_OVERVIEW DRIFT; the following detail names the delivery's missing documentation change. |
| Scannable | One bullet per changed mapped path, new directory, watched product path, or changed live journey follows the gate title. |
| Readable | The caller receives the document to update, a decision command with a reason, and a close command to retry through the existing shared upkeep renderer. |
| Machine output | Existing MCP tool errors carry the refusal; the update JSON receipt adds `projects[].phase_documents.docs_upkeep[].changes`, without extra stdout documents. |
| Omitted | Existing debt does not become a refusal; warnings and observe summaries appear in task notes and close receipts, while epic close counts escapes. |

The same renderer serves G1–G3 and G5–G6 with the caller's harness prefix. No new
CLI command or independent terminal layout is introduced. Existing updates keep
their renderer; adoption details are recorded directly in the machine receipt.

## Critique

Source and fixture checks cover the shared refusal/escape commands, snapshots and
receipt data. Terminal captures against the assembled Rust binary remain owned
by epic assembly (`cas-4cbb`); no terminal-qa PASS is claimed by this worker.
