# Machine footer: dot with the first line (cas-94eb)

- **Reader:** An operator reading the paired machine footer before opening its register.
- **Argument:** The status dot belongs to the named machine, including when its name wraps.
- **Hero form:** Existing machine evidence row; the dot and first label line share a baseline, and the status word wraps only when needed.
- **Distinctive move:** Keep the full desktop name readable in the existing footer using the house dot and spacing tokens; retain the phone's one-line name/state priority and full-name dialog.
- **Omitted:** No new badge, text, colors or connection semantics; this repairs the existing footer geometry.

Inherited language: hub-web/DESIGN.md and the existing footer tokens. The label's desktop width reserves the dot and its gap so both remain on the first flex line; the name still wraps, including an unbroken tail. Baseline alignment places the dot at the first line. Ordinary short names still share the row with their status. Phone retains its existing centering, flex layout, ellipsis and full-name title/dialog.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Existing machine evidence row and house dot; no new chrome. |
| Fit | 4 | The dot stays with the named machine through long-name wrapping and resize. |
| Hierarchy | 4 | Machine name leads, connection word follows on the same row or wraps as needed. |
| Craft | 4 | Full unbroken tail wraps inside the desktop footer; the phone retains its name/state gap. |
| Accessibility | 4 | Full accessible name and title, keyboard register and returned focus; media modes checked. |

Real production desktop/phone screenshots in light/dark were inspected. Baseline and fix raw strict probes have the same six findings: four existing JavaScript-required page findings and two deliberate phone name ellipses. The unchanged project allowlist already covers these precise cases; its existing full-name title/dialog exception is exercised by the real-build keyboard cells. Strict results and this audit stay in the task bundle. No new exception is added.
