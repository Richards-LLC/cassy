# Brief: Local network access recovery

## Single idea
Commander keeps the pairing while explaining how to allow browser access to a tailnet hub.

## Hero form
A status sentence below the session header names the machine and the browser setting. With no conversation loaded, it sits in the conversation list, which remains visible on phones, at the empty copy's 18px inset (cas-7c37f).

## Emotional register
Calm and practical: reuse the existing compatibility notice typography and theme tokens, with one concrete next step.

## Distinctive move
The same machine name used in the conversation and footer leads the permission guidance ("To reach soundwave, allow…"); transport errors stay in Details. The notice carries only the remedy: the list's empty copy or the reconnect banner owns the single "Can't reach…"/"Reconnecting…" sentence (cas-7c37f). Under forced colors it keeps a 1px CanvasText edge.

## Deliberately omitted
No Re-pair action for a browser permission denial, because replacing the active credential cannot grant network permission.

## Critique
Scored by proud-raven-98 on 2026-10-05 against the built-dist HUB-J12 permission journey at 1280×800 and 390×844, light and dark.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The house compatibility notice names soundwave, echoed in the machine footer. |
| Fit to argument | 4 | A single permission sentence sits beside the retry state and provides the action that can restore access. |
| Hierarchy | 4 | The amber notice follows the unreachable-machine explanation and stays visible with zero conversations on a phone. |
| Craft | 4 | Strict local-build inspection at rest and in the denied state adds no clipping, overlap, contrast, print-loss or overflow findings at either size/scheme. |
| Accessibility | 4 | A status live region announces changed guidance, with light/dark text contrast checked at ≥4.5:1. The existing app requires JavaScript. |

Strict visual-QA is scoped against the train-8 base: both reports contain only four existing `javascript-disabled-loss` findings on `body` (one per size/scheme), and the delivery adds none. Commander already provides a noscript instruction. The bundle carries both reports; it does not claim an unrestricted strict PASS.
