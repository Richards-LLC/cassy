### Fixed — Factory

- **A no-code spike's or demo's report always gets an independent review** — Was:
  a no-code spike whose report landed as docs in the repository was classed as a
  docs-only delivery, and a supervisor's ordinary close accepted it with no
  reviewer and no verdict. Now: a no-code spike, or a no-code task with a demo
  statement, needs an independent verdict for the report it carries. The close
  that requests the review records the proof reference and each published
  artifact's id and SHA-256 on the verification dispatch (migration m302); a
  later close compares them, so a rejected, revised or byte-changed report needs
  a new passing verdict, and nothing written in task notes changes what an
  approval covers. Mechanical no-code chores are unchanged.
