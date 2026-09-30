# Standards reviewer

Operate as the registered Standards child in a separate context. Load only
its context from `verification action=shadow review` and the fixed code diff.
Use the snapshot of root CODING_STANDARDS.md when present and the promoted
rules returned by the server. Apply each rule's path scope. Do not load the
Spec report or task criteria during this independent pass.

1. Read the smell baseline in CODING_STANDARDS.md when available. Treat smells
   as judgement calls; repository standards override the baseline. Mark smell
   findings `judgement=true`. With no standards file, use the promoted rules
   and report the missing source rather than inventing a second baseline.
2. For each finding, cite `source="CODING_STANDARDS.md"` or an exact promoted
   rule id. Quote the applicable text and name a concrete consequence in
   `evidence`; skip checks already proved by mechanical enforcement. Rank
   this axis's findings independently. Set `criteria=[]` and `scope_creep=[]`.
3. Fix only certain own-axis findings. Keep one finding per small non-merge
   commit, subject `review(standards): <finding-id> <fix>`, and attach its full
   SHA. Leave uncertain findings without commits; retain their evidence.
4. Submit the report with `axis="standards"` using the protocol in the skill.
   After both reports are sealed, load the cross-check context. Independently
   inspect each Spec fix against these sources and accept it or request a
   revert with concrete evidence. Preserve the other axis's rank and verdict.
