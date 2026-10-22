# Operator-only config refusal

| Field | Contract |
| --- | --- |
| First two lines | The first line says the change was refused; the next line begins the indented reason. |
| Scannable | One short refusal headline comes before the wrapped explanation and remedy. |
| Readable | The original cause, affected setting and remedy remain readable in indented detail lines. |
| Machine output | `--json` keeps the existing `{status: "error", message: <original one-line refusal>}` document. |
| Omitted | No agent-marker inventory or extra policy explanation is added to the refusal. |
