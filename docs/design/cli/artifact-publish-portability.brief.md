# Artifact publish portability warning

| Field | Decision |
| --- | --- |
| First two lines | A failed QA manifest upload names the bundle as not portable, while making clear the local artifact is still usable. |
| Scannable | Keep the existing verdict, filename, size and artifact ID, followed by storage state and its failure detail. |
| Readable | The portability reason explains that an uncommitted manifest cannot carry its archive to a peer; the original upload error remains visible. |
| Machine output | One JSON document retains local status and upload_failed storage while adding qa_bundle_not_portable metadata_state and a nonempty qa_portability_error. |
| Omitted | No new flags, layout, colour or success claim; storage-only matching prevents renderer panics without changing the public disposition API. |

## Critique

terminal-qa: PASS artifact-publish-manifest-failure · 12 runs · 0 fail · 0 warn · 0 allowed

Captured candidate `14cee1d6a` against a short-lived loopback fixture returning
403 for the manifest PUT after a valid QA archive was prepared. The real CLI
retains local status and the upload error while naming the bundle as not
portable. No external Cloud service or operator credentials are involved.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first line names the QA warning; the next lines preserve the local storage verdict. |
| Fit | 4 | Existing rows and the artifact-show remedy remain; no extra mode or flag is needed. |
| Craft | 5 | Human captures are at most 76 cells across 80- and 120-column runs, with no split tokens. |
| Theme safety | 5 | Four palettes, NO_COLOR and C locale pass without relying on colour. |
| Machine contract | 5 | One JSON document preserves upload_failed and adds the portability error; original 403 detail remains. |

The offline companion-error control also passes the same 12-run gate. Reports
and command captures are retained with the task's published proof artifact.
