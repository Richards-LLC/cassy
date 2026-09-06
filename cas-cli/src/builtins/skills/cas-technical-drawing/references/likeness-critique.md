# Likeness critique — is it the object?

Mechanical checks prove the projection is honest; they cannot tell whether the drawing depicts the right thing. A featureless jig passes by construction; anything with a face — furniture, a machine, a cabinet — needs this critique. Run it on the isometric sheet (and any pictorial meant to answer "what am I making?") before delivery.

## Procedure

1. **Ground truth.** The model names a reference (`reference.path`) and 3–5 identity features written at checkable resolution ("one trapezoidal notch between two flat feet", not "sculpted sides"). No reference → block the drawing; do not assess it.
2. **Plain render.** `draft.mjs render model.json --only iso --plain --png --dpi 60` produces a caption-free sheet; crop or thumbnail it to about 200 px wide (`rsvg-convert -w 200 …-plain.svg`).
3. **Cold look.** Give an agent with no context only the thumbnail and the reference-free question "What is this object, and which way up is it?" Record the answer verbatim. You cannot self-administer this step; your own context autocompletes the intended object.
4. **Feature point-check.** On the full-size render, point to each identity feature in the pixels. Count present / total.
5. **Compare against the reference side by side** and score the three axes below. Save the scorecard as `likeness.md` (or JSON) beside the drawings with the thumbnail, the cold-look answer and the feature count.

## Scorecard

| Axis | 0–3 | 4–6 | 7–8 | 9–10 |
| --- | --- | --- | --- | --- |
| **Silhouette** — category and posture read at 200 px | wrong kind of thing or lying down | right category, ambiguous posture or massing | named unprompted, one mass off | named unprompted, massing matches the reference |
| **Proportion** — envelope and major subdivisions vs the reference or plan | > 20% off | 5–20% off | < 5% off, one subdivision wrong | < 5% off throughout (the `proportion` check passes and the model matches the plan) |
| **Part identification** — a builder can name each visible part and its role without the balloons | most parts ambiguous | major parts read, details do not | all parts read; one joint or fitting ambiguous | every part and its relationship reads at a glance |

Feature point-check is reported as a count and gates separately.

## Floor to ship

- Every axis ≥ 7 and total ≥ 24 / 30.
- Feature point-check ≥ 4 of 5 (or all of 3–4 listed).
- Cold look names the correct category unprompted.

Below the floor: fix the model (massing, proportion, missing identity feature), re-render, re-run. Do not add text, colour or a caption to rescue a failing silhouette; the plain render is the test.

## Recording a result

```
Likeness — Dartboard cabinet isometric, rev B
Cold look (zero-context agent): "a tall wall cabinet with two doors and a drawer, upright"
Features: 5/5 — tall shallow box ✓, two overlay doors with panels ✓, full-width drawer ✓, frameless sides full height ✓, board centred in the opening (doors-open front) ✓
Silhouette 9 · Proportion 9 · Part identification 8 · Total 26/30 — SHIP
```
