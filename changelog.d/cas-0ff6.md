### Fixed — Factory

- **A no-code spike's or demo's report always gets an independent review** — Was:
  a no-code spike whose report landed as docs in the repository was classed as a
  docs-only delivery, and a supervisor's ordinary close accepted it with no
  reviewer and no verdict. Now: a no-code spike, or a no-code task with a demo
  statement, needs an approved independent verdict for the report it carries.
  Each review request, at close or before a merge, records the proof reference
  and the id and SHA-256 of every committed artifact on its verification
  dispatch (migration m302), the same artifacts the verifier is handed. A later
  close compares them, so a rejected, revised or republished report needs a new
  passing verdict, a skipped or dispatch-less verdict no longer closes one, and
  nothing written in task notes changes what an approval covers. Importing a
  peer's unchanged report keeps its approval. Mechanical no-code chores are
  unchanged.
