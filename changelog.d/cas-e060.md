### Added — Releases

- **Green trains cut themselves** — Was: a green release train waited for the
  supervisor to notice and run `scripts/release.sh`. Now: when the exact train
  tip has a green full-suite receipt and changelog fragments, Cassy freezes the
  train and runs `scripts/release.sh` with the supervisor's notes draft, at
  most `release.auto_cut_max_per_day` (default 4) times a day and
  `release.auto_cut_min_interval_hours` (default 2) apart, never while a
  merge-queue run is in flight or the tip is red. A missing draft wakes the
  supervisor once with a deadline; Cassy never writes or posts notes. The
  release card in Commander has **Pause auto-cut** and **Resume auto-cut**.
