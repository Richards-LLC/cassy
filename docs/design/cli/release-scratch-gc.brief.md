# Release scratch in factory GC

| Field | Contract |
| --- | --- |
| First two lines | The existing GC report gains reclaimable, reclaimed and retained byte counts for owned release scratch and assembly caches. |
| Scannable | Scratch and cache paths remain visible alongside their admission or retention reasons. |
| Readable | Unknown provenance, live leases and registered remaps explain why bytes remain; cleanup uses the existing explicit GC mutation gate. |
| Machine output | `RELEASE_SCRATCH_STATUS_JSON` contains entries, caches and byte totals; the standalone helper prints exactly one JSON document without color. |
| Omitted | This task adds an inventory seam, preserving the existing factory GC layout; full runtime terminal captures wait for the supervisor's build. |

## Verification

Python fixtures verify JSON parsing, retained legacy-cache byte accounting and
read-only inventory. Runtime GC rendering is unverified until the supervisor
builds the Rust delivery; no terminal QA receipt is claimed from a source stub.
