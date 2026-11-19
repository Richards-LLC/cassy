### Fixed — Release trains

- **Move unfinished work when a train is cut** — Cutting a release opens and publishes its successor, retargets unfinished children, and tells their assigned workers the new lane. Parked tips that contain the successor base keep their anchors and proof; other tips receive one merge-then-repark instruction. Close now refuses an unmergeable cut or frozen target before proof, CI or verification work, with `G-100` and the successor identity. Published delivery bookkeeping and red release repairs remain admitted.
