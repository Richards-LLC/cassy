# Brief: Machine Interlock

## Idea and operator value

Commander becomes a spatial machine map: machine ports feed project supervisors, whose worker branches show where a decision is holding the flow. The operator chooses at the relevant junction, seeing the supervisor’s stated next step along the outgoing branch. A connection gap breaks a separate health conductor, not the work path. This is useful when supervisors delegate across a fleet: ownership and consequences stay attached to the right machine, without a maze of conversations or a generic workflow-builder canvas.

## Single idea

An answer belongs to one junction in the machine–supervisor–worker hierarchy, and its next step must be stated or unknown.

## Hero form

Annotated network diagram (an intentional extension to the report form vocabulary for a product control surface): machine identity anchors the entry, supervisor requests sit at junctions, and worker branches carry explicit observed or proposed next-step annotations. The main shape is a connected topology, not a set of node cards. Phone compresses to the addressed junction with the other machines’ health still visible.

## Emotional register

Precise, hands-on: continuous conductors, readable destination labels, mechanical-looking switching geometry and an open question at the focal junction. No glow, dial gauge or pseudo-safety certification.

## Distinctive move

The answer controls are the two exits of the request junction; the receipt witness sits on the selected route and identifies the exact addressed supervisor.

## Deliberately omitted

No rectangular node-card workflow editor, dragging required for control, arbitrary network map, left sidebar, swatches, status pills or inferred Done endpoints. Decorative wiring cannot invent actual dependencies.

## Commander-specific data requirements

Current machine/project/session catalog plus exact supervisor-worker roster supplies hierarchy. It does not prove cross-machine work dependencies; only draw a dependency if the hub reports it explicitly. Current action may be absent for unvisited conversations and must be labeled unknown. Choice-specific downstream branches require a supervisor-authored next-step/dependency payload, including identities; otherwise the branch ends at “Next step not supplied”. A delivered answer does not prove a worker resumed: draw that continuation only after an observed action event. Connection cause and heartbeat freshness remain independent. Trust evidence is forwarded, stored on this device and read, never a single all-clear.

## Reference

[Network Rail’s signalling explanation](https://www.networkrail.co.uk/stories/signals-explained/) describes routes and the relationship between points and signals. [Its signaller account](https://www.networkrail.co.uk/stories/whats-it-like-to-be-a-signaller/) describes an operator setting routes and observing track circuits. The design inference is a control surface that exposes destination and state at the point of action; Commander will not claim railway-style safety interlocking or infer work dependencies from a drawing.

## Boundaries and data honesty

This is a static concept study with clearly labeled sample data, not a running Commander delivery. The current hub has no Done session state. An unvisited conversation may have no current action; show “Current action not observed” rather than fabricate work. Connection observations remain independent of work and completion. A heartbeat is not activity; a quiet worker is not disconnected. Receipt words distinguish forwarded, stored on this device and read; absence of read evidence remains unknown. Unconfirmed sends do not silently become forwarded.

Actual elapsed blocking requires a durable hub waiting-since timestamp, with clock provenance. Until supplied, the UI can say “Seen on this device 7m ago” but cannot claim “blocked for 7m”. Choice consequences are quoted only when the supervisor supplies them; otherwise show “Next step not supplied”. No predictive completion or release state is inferred from an answer.

## Render contract

Self-contained HTML, no network/assets required, real project token names inherited from `hub-web/src/tokens.css`; neutral overrides need a stated brand reason. The operator’s explicit ban overrides the house serif-hero default: sans for working questions and mono for IDs/timing, no serif hero. No list-plus-detail card shell, left accent bars or swatches, status pills, gradient glow, rounded SaaS cards or Terminal view. A single vivid action accent belongs to the decision/control geometry, never a row’s left edge.

The operator review package requires desktop-dark at1280×800 and Pixel phone-light at390×844 for each concept. Craft validation also renders the other two scheme/viewport combinations, JS-off, print and reduced-motion. The core loop and native44px thumb controls must fit the first phone screen; the footer may not conceal the decision. Print preserves question, provenance and result. Motion is optional illustration, never the only signal; reduced motion gets the same evidence. Each diagram has a text alternative and supporting data table. Critique is appended after rendering; no score is claimed now.
