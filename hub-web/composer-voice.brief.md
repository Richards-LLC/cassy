# Brief: conversation composer voice control

## Single idea
The composer keeps the operator in control: a roomy draft field is the primary surface, while a quiet mic becomes a clear listening state and never sends speech by itself.

## Hero form
Inline control row: the field expands between fixed attach, mic, and send affordances so the geometry makes review-first dictation visible at a phone width.

## Emotional register
Calm, legible, deliberate — PAPER Slate neutrals carry idle controls, the accent fill is reserved for active listening, and the editable draft remains the largest object in the row.

## Distinctive move
The mic is a compact round state switch: its outline becomes a softly pulsing filled accent while listening, then returns to idle after placing the transcript at the caret.

## Deliberately omitted
No persistent “Voice ready” sentence or wide “Tap to talk” pill: capability and permission failures live on the mic itself so the composer does not reflow.

## Critique

| Dimension | Score | Evidence |
|---|---:|---|
| Distinctiveness | 4 | Round mic uses the existing Pebble token language and reserves the accent fill plus pulse for listening. |
| Fit to argument | 5 | The text field measures 234px of the 390px row (0.600); the mic’s visual state communicates listening without competing with review. |
| Hierarchy | 5 | The draft is the widest control; attach, mic, and send are equal-sized secondary affordances, and no voice hint row remains. |
| Craft | 5 | `visual-qa.mjs --strict` PASS: 26 fixtures × 2 schemes × 3 viewports, 0 findings, 156 screenshots; real-build captures show no phone overflow. |
| Accessibility | 5 | Mic has an icon plus changing `aria-label`, `aria-pressed`, title, and `aria-description`; unsupported/denied states stay disabled and reduced motion removes the pulse. |

Scored by Codex on 2026-09-21; floor holds.
