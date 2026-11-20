## Core Concepts

- **Cancelled lane CI re-runs once** - when a parked delivery's lane run is cancelled (a job past its 45-minute limit reports cancelled and nothing replaces it), the armed automatic merge re-runs that run once, keeps waiting while the original cancellation is still the latest result, and merges on green. It stops with the existing notice only when the re-run is cancelled too.
