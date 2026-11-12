### Changed — Merge admission

- **One origin read per merge** — Was: a merge asked origin about its target up to four times, and a three-entry train merge made 24 origin reads (8 per entry). Now: the fetch under the target lock is the only merge read. The target sync, its containment re-check, the train's publication check, promotion recovery and the close's integration proof reuse it. The close reads origin only when the tracking ref cannot prove the delivery, and then at most once. The same train makes 3 reads (1 per entry). A new pre-merge review still refreshes the target before it is dispatched.
- **Session ownership checked once** — Was: the foreign-session check ran before and after parked-delivery inference. Now: it runs once, after inference, and still comes before any effect or inference refusal.
- **Registry** — The containment-freshness code G-105 is folded into G-110. Both reported the target's freshness from the same read. The registry drops from 401 to 400 checks.
