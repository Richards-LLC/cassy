### Fixed — Factory

- **A no-code spike's or demo's report always gets an independent review** — Was:
  a no-code spike whose report landed as docs in the repository was classed as a
  docs-only delivery, and a supervisor's ordinary close accepted it with no
  reviewer and no verdict. Now: a no-code spike, or a no-code task with a demo
  statement, needs an independent verdict bound to the artifact ids and SHA-256
  digests it reviewed; a rejected, revised or byte-changed report needs a new
  passing verdict. Mechanical no-code chores are unchanged.
