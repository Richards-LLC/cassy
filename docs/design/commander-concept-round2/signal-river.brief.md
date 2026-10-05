# Brief: Signal River

## Idea and operator value

Commander becomes a live instrument: separate traces show each machine’s connection beats and the work passing through its supervisor to its workers. A request appears as a break in a work branch; the operator answers at that point, rather than finding it in a sidebar. This is useful when several machines are busy: one can distinguish a connection gap from work waiting for a decision, compare time lost, and keep the answer beside the evidence that prompted it.

## Single idea

The visible bottleneck is the branch waiting for my answer; the machine’s heartbeat is a separate observation.

## Hero form

Annotated timeline with small-multiple machine traces: horizontal time on desktop and vertical time on phone, with supervisor/worker branches and direct event labels. The question interrupts the relevant branch, so the geometry explains why answering matters.

## Emotional register

Watchful, measured: etched timing ticks, quiet mono event labels, strongly legible traces and one unambiguous open question. No theatrical waveform masquerades as work.

## Distinctive move

The question occupies the actual interruption in a time trace; its answer reconnects that trace visually, with a separate receipt witness.

## Deliberately omitted

No conversation list, card stack, KPI tiles, swatch or serif headline. No busy pulse for unknown work. Missing heartbeat observations create labeled gaps, while missing action history stays visibly unknown.

## Commander-specific data requirements

Existing device connection snapshots can label the most recent observed success, misses and named causes. Existing received asks supply the question and declared choices. A trustworthy historical river needs a bounded ordered activity-event feed with machine/project/session/agent identity, event kind, timestamp/provenance and sequence. The current catalog’s last activity value does not provide this history; until that feed exists, show only collected device observations and explicitly mark earlier/unvisited spans unknown. Exact blocker duration needs hub waiting-since. Any downstream resume/release effect needs an explicit supervisor next-step statement or later observed event.

## Reference

[NASA’s Apollo Mission Control restoration](https://www.nasa.gov/johnson/history/apollo-mcc-restoration/) documents the authentic monitoring consoles and group displays. The design inference is an instrument whose live observations are legible at a glance; it does not copy CRT styling or borrow NASA authority for Commander data.

## Boundaries and data honesty

This is a static concept study with clearly labeled sample data, not a running Commander delivery. The current hub has no Done session state. An unvisited conversation may have no current action; show “Current action not observed” rather than fabricate work. Connection observations remain independent of work and completion. A heartbeat is not activity; a quiet worker is not disconnected. Receipt words distinguish forwarded, stored on this device and read; absence of read evidence remains unknown. Unconfirmed sends do not silently become forwarded.

Actual elapsed blocking requires a durable hub waiting-since timestamp, with clock provenance. Until supplied, the UI can say “Seen on this device 7m ago” but cannot claim “blocked for 7m”. Choice consequences are quoted only when the supervisor supplies them; otherwise show “Next step not supplied”. No predictive completion or release state is inferred from an answer.

## Render contract

Self-contained HTML, no network/assets required, real project token names inherited from `hub-web/src/tokens.css`; neutral overrides need a stated brand reason. The operator’s explicit ban overrides the house serif-hero default: sans for working questions and mono for IDs/timing, no serif hero. No list-plus-detail card shell, left accent bars or swatches, status pills, gradient glow, rounded SaaS cards or Terminal view. A single vivid action accent belongs to the decision/control geometry, never a row’s left edge.

The operator review package requires desktop-dark at1280×800 and Pixel phone-light at390×844 for each concept. Craft validation also renders the other two scheme/viewport combinations, JS-off, print and reduced-motion. The core loop and native44px thumb controls must fit the first phone screen; the footer may not conceal the decision. Print preserves question, provenance and result. Motion is optional illustration, never the only signal; reduced motion gets the same evidence. Each diagram has a text alternative and supporting data table. Critique is appended after rendering; no score is claimed now.
