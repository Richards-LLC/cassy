# Brief: Code frames

## Single idea

Every fenced source block is a small reading tool: its identity and Copy and Wrap controls remain visible on a phone.

## Hero form

Annotated specimen: a mono filename leads a ruled header directly above its selectable source, with two named actions alongside it.

## Emotional register

Calm, precise: opaque Glass reading tokens, quiet mono labels and stable text controls keep the code primary.

## Distinctive move

The mono source label anchors a compact tool strip; the violet pressed Wrap state makes the reading mode visible without competing with the source.

## Deliberately omitted

No Run action, hover-only controls or command inference: this frame owns reading and exact copying, while rich previews own their content. Optional lazy source color extends this frame in [code-syntax.brief.md](code-syntax.brief.md).

## Implementation boundary

`createCodeFrame(document, { source, language, filename })` owns only local Copy state and Wrap state. It exposes its content container and pre node so rich renderers can replace content while the header and original copy source survive. No global storage or remote state. Tokens come from `hub-web/DESIGN.md`, `tokens.css` and `glass.css`.

## Critique

Scored by the implementation worker on 2026-10-28 from strict four-view captures and the phone reduced-motion capture. Final receipts live under the task artifact directory.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Glass reading surface and a mono source label anchor the violet pressed Wrap tool. |
| Fit | 5 | The source immediately follows its identity and both reading actions. |
| Hierarchy | 4 | Source owns the frame; the label and two quiet tools share a single header at 390px. |
| Craft | 4 | Strict visual QA PASS: 1280×800 and 390×844 in light and dark, zero findings. |
| Accessibility | 4 | Native keyboard controls, pressed state, polite status, scrollable source, print wrapping and forced-color/reduced-motion/more-contrast captures; Commander declares its JavaScript requirement. |
