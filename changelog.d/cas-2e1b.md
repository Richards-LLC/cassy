### Changed — CI

- **Lane CI runs the delivery's impacted tests** — Was: a factory push proved
  only the tests its worker named; the train's impact selection ran after
  merge, so a change that broke a test outside its own module showed up as a
  train red about 35 minutes later. Now: the slim lane runs that same
  selection for the delivery's range, from the start base recorded at task
  start, on hosted runners, the changed modules'
  own tests first and the remaining selected tests in up to three shards only
  after those pass. When the selector cannot narrow a change (build policy,
  scripts or workflow edits), the lane runs the whole suite in the same order.
  A test that already fails on the branch the delivery started from is named
  as inherited and does not block that delivery.
