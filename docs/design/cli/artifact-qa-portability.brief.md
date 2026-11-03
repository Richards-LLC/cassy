# Artifact QA portability warning

| Field | Contract |
| --- | --- |
| First two lines | A failed QA archive starts with `[WARN] QA bundle not portable`, followed by the manifest's actual storage result and the archive failure. |
| Scannable | Keep the artifact name, size and id in the existing verdict row; the digest and ordinary storage recovery remain below it. |
| Readable | Preserve the archive failure reason, including size and containment refusals, instead of hiding it in tracing logs. |
| Machine output | One JSON document preserves `status`, `storage` and `storage_detail`; `metadata_state` becomes `qa_bundle_not_portable` and `qa_portability_error` carries the reason. MCP leads with the same warning. |
| Omitted | Per-part-only receipts and traces stay on the producer host; files named by the gate's top-level fields still travel, including aliases into `parts/`. |

Design: `DESIGN.md`; ASCII, uncoloured warning follows the existing artifact
command. A successful manifest upload retains its Cloud storage result, while
its evidence warning takes the headline. Rebuild and republish the bundle
before a peer closes the task.

## Critique

Source review: hierarchy 4 (portability warning first), fit 4 (existing rows,
wrapped summary), craft 4 (storage and portability stated separately), theme
safety 5 (uncoloured ASCII), machine contract 5 (typed reason and existing
storage fields). Real-build terminal capture is deferred to the supervisor's
combined batch QA candidate; no terminal-qa PASS is claimed here.
