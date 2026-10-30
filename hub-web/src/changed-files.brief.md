# Brief: per-turn changed files

## Single idea
The operator can scan a turn's tool-reported edits before deciding which source evidence to open.

## Hero form
Callout rail: one compact disclosure beneath the turn's work summary, followed by a ruled file list only when requested.

## Emotional register
Quiet, inspectable — Glass raised surface, muted provenance, mono paths and native keyboard controls.

## Distinctive move
The file path is the disclosure: opening it reveals the recorded unified diff or the original edit evidence in place, with focus and expansion surviving updates.

## Deliberately omitted
No invented line counts or Git verdict: v2 reports successful tool calls and explicitly leaves shell side effects unknown. No patch wall, split view or review comments.

## Component and state boundary
The thread model selects the newest snapshot per session/agent/turn and places it once in the latest work block for that turn. Work rows render the card with the existing unified diff viewer. WorkPicture owns device-local disclosure state and lazy source detail requests. Unknown, loading, missing and failed evidence remain explicit.
