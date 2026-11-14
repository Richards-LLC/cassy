### Fixed — Factory

- **Worker capacity names every project on the machine** — Was: a spawn
  refused at "8/8 live on this machine" while `worker_status` listed one live
  worker, because the limit counts other projects' workers too. Now: the
  refusal and `worker_status` show the same machine-wide count with a
  per-project breakdown, for example "8/8 live on this machine: cassy 1,
  gabber-studio 7".
