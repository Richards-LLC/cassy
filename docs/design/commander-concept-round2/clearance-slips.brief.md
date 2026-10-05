# Brief: Clearance Slips

## Idea and operator value

Commander becomes a physical clearance rack: each unanswered decision is a narrow operational slip carrying the machine, project, supervisor, exact question and the stated next step. The oldest known request is pulled forward; its two answer controls live on the slip itself. After an answer, a small receipt remains attached to that same slip. This is useful when attention is scarce: the unit of navigation is a decision one can finish, with enough context to avoid answering the wrong machine.

## Single idea

A decision is a clearance I can issue once, carrying its destination and its evidence with it.

## Hero form

Operational strip rack (an intentional extension to the report form vocabulary because the unit of control is a decision): waiting slips are ordered by observed age, and the foremost slip opens along its length into the declared consequence and choices. Desktop uses wide ruled flight-strip geometry; phone uses one folded slip spanning the thumb area. It is a queue of action objects, without a list/detail pane or card-dashboard shell.

## Emotional register

Tactile, accountable: tight ruled fields, die-cut paper ends, mono destination numbers, restrained hard-edged depth and a stamped receipt. Decoration never implies a machine is healthy.

## Distinctive move

The exact destination, choice and later receipt are physically retained on the same clearance slip rather than dissolved into a toast.

## Deliberately omitted

No kanban columns, Done pile, left status stripe, pill, hero type treatment or flat card grid. The paper is an operational object, not a skin for the old conversation list. No pull gesture is required to send; both visible choices are one tap.

## Commander-specific data requirements

Received asks and declared choices are enough for the decision object. Destination joins use hub/project/session/agent identity and exact catalog roster membership. Age is explicitly “seen here” unless a durable waiting-since is supplied by the hub. Each choice’s consequence requires supervisor-authored text; missing consequence reads “Next step not supplied”. Connection health is a separate narrow fleet witness, using named observation causes; unvisited action stays unknown. Forwarded/device-stored/read evidence remains three distinct observations, and a receipt never claims task completion.

## Reference

[FAA’s time-series study of flight progress strips](https://www.faa.gov/sites/faa.gov/files/data_research/research/med_humanfacs/oamtechreports/AM95-04.pdf) examines their role in air traffic control. [EUROCONTROL’s initial integrated tower working-position study](https://www.eurocontrol.int/sites/default/files/library/040_ITWP_Initial_Study.pdf) describes strip arrangement, annotations and handoffs. The design inference is that a compact, movable work object can preserve context through a handoff; the mockup does not simulate aviation procedures.

## Boundaries and data honesty

This is a static concept study with clearly labeled sample data, not a running Commander delivery. The current hub has no Done session state. An unvisited conversation may have no current action; show “Current action not observed” rather than fabricate work. Connection observations remain independent of work and completion. A heartbeat is not activity; a quiet worker is not disconnected. Receipt words distinguish forwarded, stored on this device and read; absence of read evidence remains unknown. Unconfirmed sends do not silently become forwarded.

Actual elapsed blocking requires a durable hub waiting-since timestamp, with clock provenance. Until supplied, the UI can say “Seen on this device 7m ago” but cannot claim “blocked for 7m”. Choice consequences are quoted only when the supervisor supplies them; otherwise show “Next step not supplied”. No predictive completion or release state is inferred from an answer.

## Render contract

Self-contained HTML, no network/assets required, real project token names inherited from `hub-web/src/tokens.css`; neutral overrides need a stated brand reason. The operator’s explicit ban overrides the house serif-hero default: sans for working questions and mono for IDs/timing, no serif hero. No list-plus-detail card shell, left accent bars or swatches, status pills, gradient glow, rounded SaaS cards or Terminal view. A single vivid action accent belongs to the decision/control geometry, never a row’s left edge.

The operator review package requires desktop-dark at1280×800 and Pixel phone-light at390×844 for each concept. Craft validation also renders the other two scheme/viewport combinations, JS-off, print and reduced-motion. The core loop and native44px thumb controls must fit the first phone screen; the footer may not conceal the decision. Print preserves question, provenance and result. Motion is optional illustration, never the only signal; reduced motion gets the same evidence. Each diagram has a text alternative and supporting data table. Critique is appended after rendering; no score is claimed now.
