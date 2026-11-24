### Fixed — Factory

- **A no-code spike's or demo's report always gets an independent review** — Was:
  a no-code spike whose report landed as docs in the repository was classed as a
  docs-only delivery, and a supervisor's ordinary close accepted it with no
  reviewer and no verdict. Light depth, a rapid-response window, a release tag
  containing the delivery, `supervisor_override` and `task update status=closed`
  each skipped the review as well. Now: one gate runs before every exemption on
  close, `task update status=closed`, pre-merge review and release close. A
  no-code spike, or a no-code task with a demo statement, closes or merges only
  on an approved verdict whose dispatch recorded the proof reference and the id
  and SHA-256 of every committed artifact (migration m302), and only while that
  record still equals the report. A task stays covered once a review has
  recorded its proof, even if its fields are edited in the same request. The
  verifier is handed the recorded reference and artifacts, never a later
  listing. A changed, republished or revoked report retires the stale dispatch
  and the next close or merge requests a new review; a temporary download
  failure retries the same dispatch. A revocation Cloud reports is recorded
  locally, so that new review leaves the artifact out. The approval is checked
  again at the final write of close, `task update status=closed` and release
  apply and at merge admission, so a report edited, republished or revoked
  while one of them runs is refused; a release preview changes nothing. An
  unreadable inventory, dispatch or verdict refuses with G-68. `supervisor_override` and the shipped-timeout
  waiver no longer close a report: a supervisor records a verdict on the
  dispatch instead. Negative-result, evidence-only and decision closes, the
  operator recovery decision and turning verification off are unchanged, as are
  mechanical no-code chores.
