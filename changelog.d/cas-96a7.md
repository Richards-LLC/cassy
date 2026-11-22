### Fixed — Factory

- **Brief task start shows the close checks again** — Was: the predicted QA
  impact table was added to the start-only advisories, so a brief `task
  action=start` often went over its response budget and replaced the close
  checks with a pointer to the saved notes. Now: the prediction is reference
  context. It is still saved with the start notes and shown in a full start,
  but in a brief start it is clipped with the other context, and the close
  checks stay inline. The response budget is unchanged.
