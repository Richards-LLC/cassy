# Brief: Evidence tables

## Single idea

A supervisor's evidence stays in real columns when read by a screen reader or copied into a spreadsheet.

## Hero form

An annotated specimen: a native table on the existing Glass sheet, with two visible copy actions above the columns.

## Emotional register

Quiet and precise: prose reads in Inter, counts and results align in mono, and named outcomes keep their existing green, amber and red tones.

## Distinctive move

The compact Copy Markdown / Copy CSV strip lets the reader choose between the original message source and spreadsheet cells without leaving the conversation.

## Deliberately omitted

No sorting, pagination, download dialog or global clipboard settings: the supervisor's row order is evidence, and the table owns only local copy feedback.

## Implementation boundary

`messageBlocks` classifies tables outside fences and retains their source. `renderBody` builds native table/header/body/cell nodes, pads ragged rows and owns clipboard success/failure/retry. `tableCsv` quotes plain cell strings; Markdown copy retains source separators and formatting. All state is local to the rendered table. Tokens come from `hub-web/DESIGN.md@3c25c3fb3`, `tokens.css` and `glass.css`; existing branding, font assets, shell and motion remain inherited. Native controls and focusable overflow provide keyboard access; the status region announces copy outcomes. Commander requires JavaScript.
