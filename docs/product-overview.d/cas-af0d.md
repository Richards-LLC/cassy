## Core Concepts

A team task push rejected for a stale revision is recovered by one pull. The pull adopts the cloud's current revision and rebuilds the queued push from the local row, including when the local row is a parked or closed delivery that outranks the cloud's older Open state. A pull blocked by a store veto or a row owned by another project still leaves the push parked as an explicit conflict.
