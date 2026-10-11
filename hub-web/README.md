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

The terminal adapter is pinned from `pingdotgg/t3code` commit
`05eb051184ac4d486795ac6f8be29129b8b8845f`, using Ghostty revision
`9f62873bf195e4d8a762d768a1405a5f2f7b1697` and Zig 0.15.2. The two WASM integrity hashes are:

- `ghostty-vt.wasm`: `6b1df1a96d59adc26360c312924898dbc122f980c17a32eb1624e48795b83f7e`
- `ghostty-write-pty.wasm`: `75cb147e98ede3f85f3cd6236a30f6d12565b0b237e1d8db941f5f3e8ad3d903`

`TerminalSurface` and `TerminalSurfaceFactory` in `src/terminal.ts` are the swappable renderer boundary. Since the Terminal view was removed (cas-0546) the surface runs only as the hidden emulator behind the conversation's read-only Raw output drawer.
The vendored T3 Code, Ghostty, and symbols-font MIT notices are retained alongside their assets.
