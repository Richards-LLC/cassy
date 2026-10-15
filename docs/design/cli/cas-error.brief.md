# CLI error lines

| Field | Contract |
| --- | --- |
| First two lines | A command failure opens with one red error mark and its message; a `CasError` suggestion follows only when available. |
| Scannable | The error mark is visually distinct from the message, with any suggestion label and remedy beneath it. |
| Readable | The mark uses the foreground-safe `text_error` token; the message stays in the terminal foreground, and C locale uses `[ERROR]`. |
| Machine output | Human errors go to stderr; JSON remains one stdout document and existing exit codes carry the result. |
| Omitted | No stack trace, extra metadata, or new error advice is added; suggestions remain limited to `CasError` values. |
