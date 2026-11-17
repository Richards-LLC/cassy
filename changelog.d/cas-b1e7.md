### Fixed — Factory

- **Workers can send quiet messages** — Was: role contracts told workers to send FYIs with `wake=false`, but the `coordination` tool did not publish that parameter, so strict-schema clients rejected it. Now: `coordination` publishes `wake`, and the supervisor `factory` tool no longer lists it.
- **Context checkpoints recycle the worker** — Was: the worker launch contract said to send `kind=status` with `need: respawn`, which only woke the supervisor. Now: it names `kind=respawn`, which queues the recycle with the worker's recipe and task bindings.

### Changed — Agent instructions

- **Instructions match the current release flow and are shorter** — Was: builtin skills and docs described the release audit running twice, omitted that legacy holds outside a release never refuse its cut, and repeated launch-contract rules in several files. Now: they describe the digest-verified Linux release, frozen-train repair admission, full-set receipt reuse, six Rust shards and jobless-run replacement, and each rule lives in one place.
