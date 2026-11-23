### Fixed — Task close

- **Operations-only epics close normally** — Was: an epic whose children all
  closed without code, with an approved epic verification, was refused by the
  pre-close structural lint when its branch was the repository's root commit,
  and no supported close remained. Now: such an epic closes by the ordinary
  path with an audit note naming the approved verdict; any child code delivery
  still runs the lint and keeps its refusal.
