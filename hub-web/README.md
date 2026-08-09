# CAS Commander web client

Commander is a controller-origin SPA embedded in `cas hub`. Build it with `npm ci && npm run build`.
The checked-in `dist/` is the Cargo input so ordinary Rust builds remain offline and do not require Node.

The terminal adapter is pinned from `pingdotgg/t3code` commit
`05eb051184ac4d486795ac6f8be29129b8b8845f`, using Ghostty revision
`9f62873bf195e4d8a762d768a1405a5f2f7b1697` and Zig 0.15.2. The two WASM integrity hashes are:

- `ghostty-vt.wasm`: `6b1df1a96d59adc26360c312924898dbc122f980c17a32eb1624e48795b83f7e`
- `ghostty-write-pty.wasm`: `75cb147e98ede3f85f3cd6236a30f6d12565b0b237e1d8db941f5f3e8ad3d903`

`TerminalSurface` and `TerminalSurfaceFactory` in `src/terminal.ts` are the swappable renderer boundary.
The vendored T3 Code, Ghostty, and symbols-font MIT notices are retained alongside their assets.
