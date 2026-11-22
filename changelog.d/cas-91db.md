### Fixed — Task reads

- **Reject unsupported list offsets** — Task list previously accepted `offset` and repeated the first page. It now returns `INVALID_PARAMS` with guidance to use filters or a larger limit. Show, get and notes retain their lossless byte cursors.
