# Concept brief

Write the brief before the first line of markup and commit it beside the artifact as
`<artifact-basename>.brief.md`. It is the design decision, recorded once; the critique appends to it.

## Template

```markdown
# Brief: <artifact filename>

## Single idea
<One sentence. What the reader must believe after three seconds. Not a topic — a claim.>

## Hero form
<The form from form-vocabulary.md that carries the idea above the fold, and why that form's
shape *is* the claim (e.g. "slope chart: the fall from 31% to 9% is the whole story").>

## Emotional register
<Two or three words that a reader would use, and the design choices that earn them
(e.g. "calm, certain — sandstone hero, serif verdict, no status colour above the fold").>

## Distinctive move
<The one compositional or typographic decision a reader would remember; something a template
would not have done (e.g. "the timeline runs down the margin column and the prose annotates it").>

## Deliberately omitted
<What a default render would have included and this one refuses, with the reason
(e.g. "no KPI card row: four numbers in boxes show no argument").>

## Critique
<Appended after render — the scored table from critique-rubric.md.>
```

## Tests a filled field must pass

- **Single idea** names a subject and a direction. "Latency review" fails; "One deploy, not a
  trend, caused the regression" passes.
- **Hero form** is a name from the vocabulary plus the reason its geometry matches the idea. A
  form chosen because it looks rich fails.
- **Emotional register** is earned by named choices; adjectives alone fail.
- **Distinctive move** is one thing. Three moves is a template with flourishes.
- **Deliberately omitted** names at least one thing a safe render would have shipped.

A brief whose fields could be pasted under another artifact is unfilled. Rewrite it.
