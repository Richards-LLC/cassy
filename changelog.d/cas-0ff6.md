### Fixed — Factory

- **A no-code spike's or demo's report always gets an independent review** — Was:
  a no-code spike whose report landed as docs in the repository was classed as a
  docs-only delivery, and a supervisor's ordinary close accepted it with no
  reviewer and no verdict, and a light-depth one skipped review altogether. Now:
  a no-code spike, or a no-code task with a demo statement, needs an approved
  independent verdict for the report it carries, at any depth. Each review
  request, at close or before a merge, records the proof reference and the id
  and SHA-256 of every committed artifact on its verification dispatch
  (migration m302), and the verifier is handed exactly that recorded set, so a
  report published after the request is not reviewed under it. A later close
  compares them, so a rejected, revised, republished or revoked report needs a
  new passing verdict, a skipped, dispatch-less or unreadable verdict no longer
  closes one, `task update status=closed` refuses one, and nothing written in
  task notes changes what an approval covers. Importing a peer's unchanged report keeps its approval throughout the
  import. Mechanical no-code chores are unchanged.
