# Cassy Commander web client

Cassy Commander is a controller-origin SPA embedded in `cas hub`. Build it with `npm ci && npm run build`.
The checked-in `dist/` is the Cargo input so ordinary Rust builds remain offline and do not require Node.
Merges never hand-merge `dist/`: `.gitattributes` marks it `merge=cas-generated`, so a merge keeps the
target's copy without a conflict, and `scripts/regenerate-generated-artifacts.sh <pre-merge-head> <merged-head>`
rebuilds and commits it from the merged sources. Cassy's merges register the driver and run the script; after a
manual `git merge`, run `python3 scripts/cas-merge-drivers.py install` once per clone and then the script.

Page-initiated pairing uses one explicit external relay boundary. The reviewed
`cas-pairing-relay-origin` metadata in `index.html` is
`https://petra-stella-cloud.vercel.app`; create, poll, and acknowledge requests
go there with credentials omitted. They never resolve against the controller
hub or the optional static host. The invitation exchange and every authenticated
`/v1/*` request or WebSocket remain direct browser-to-target-hub traffic. If the
metadata is absent or is not an HTTPS origin, Cassy Commander hides the create action
and legacy `cas hub pair` fragments remain available. Changing the relay origin
requires a reviewed source and `dist/` rebuild rather than deployment-time HTML
mutation.

Commander runs no terminal emulator (cas-f285). The conversation mounts straight into its stage,
and the read-only Raw output drawer reads the supervisor pane's rows as the factory daemon renders
them, a bounded page at a time (`ScrollbackRequest` → `ScrollbackPage`, `src/raw-output.ts`). A daemon
too old to announce paged rows falls back to the pane's recent output with its control sequences removed.
The Ghostty WebAssembly terminal, its T3 Code adapter and the symbols font left the bundle with it.
