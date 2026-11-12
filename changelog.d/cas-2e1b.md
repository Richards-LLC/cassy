### Changed — CI

- **Lane CI runs the delivery's impacted tests** — Was: a factory push proved
  only the tests its worker named; the train's impact selection ran after
  merge, so a change that broke a test outside its own module showed up as a
  train red about 35 minutes later. Now: the slim lane runs that same
  selection for the delivery's range on hosted runners, the changed modules'
  own tests first and the remaining selected tests in up to three shards only
  after those pass. Deliveries with no provable scope (build policy, scripts
  or workflow changes) still rely on the train suite.
