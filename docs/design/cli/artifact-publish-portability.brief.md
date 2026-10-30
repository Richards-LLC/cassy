# Artifact publish portability warning

| Field | Decision |
| --- | --- |
| First two lines | A failed QA manifest upload names the bundle as not portable, while making clear the local artifact is still usable. |
| Scannable | Keep the existing verdict, filename, size and artifact ID, followed by storage state and its failure detail. |
| Readable | The portability reason explains that an uncommitted manifest cannot carry its archive to a peer; the original upload error remains visible. |
| Machine output | One JSON document retains local status and upload_failed storage while adding qa_bundle_not_portable metadata_state and a nonempty qa_portability_error. |
| Omitted | No new flags, layout, colour or success claim; storage-only matching prevents renderer panics without changing the public disposition API. |

## Critique

Pending terminal capture against the named candidate.
