# Browser receipts

Open `../index.html` directly: every image, style and interaction is embedded. No server is needed to review it.

The proposed Hub screens are authored illustrations. The baseline is a real browser capture of the committed current Commander, with one actual read-only paired Hub and a clearly labeled simulated second host. Raw PNGs are embedded in the HTML; `current-state.json` records the eight capture times, viewports and source revisions. No credentials or browser storage are committed.

The interaction receipts cover 390×844 portrait, 844×390 landscape and 1280×800, light and dark. Full browser artifacts (including PDF, screenshots, JSON and the eight-cell evidence ledger) live at `/home/pippenz/.cas/artifacts/cas-cc1b/`. The committed receipt files identify the exact HTML SHA-256. The browser run tests the real generated file; current-state transport fixtures do not participate in reader QA.

## Reproduce

From the repository root, with Node, Playwright and Chromium available:

```bash
node docs/design/hub-mobile/build.mjs
node docs/design/hub-mobile/check-drawings.mjs /path/to/artifacts
node docs/design/hub-mobile/qa.mjs file:///absolute/repo/docs/design/hub-mobile/index.html /path/to/artifacts/qa
node scripts/visual-qa.mjs --strict --artifact-dir /path/to/artifacts/visual-qa file:///absolute/repo/docs/design/hub-mobile/index.html
```

`build.mjs` reads `index.md` and reuses the already embedded current-state images; it needs no external artifacts after the first generation. Set `PLAYWRIGHT_MODULE` to an installed Playwright package directory if discovery cannot find it, and `CHROMIUM_PATH` for an alternate Chromium executable.

For a fresh baseline, register `python3 docs/design/hub-mobile/serve-current.py --port 8417` using Cassy `coordination action=server_start`, then mint a temporary read-only `cas hub pair --origin http://127.0.0.1:8417 --scopes machine:read,session:read,pane:read --json` invitation into a private scratch file. Pass that file and the capture artifact directory to `capture-current.mjs`; then pass the directory to `build.mjs`. The local live Hub address in that script is `http://127.0.0.1:4173`. Revoke the temporary device with `cas hub auth revoke DEVICE_ID` and stop the registered server after capture. The script pairs and captures in one browser context; restoring serialized browser storage was not reliable in the initial capture experiment.

## Limits

Chromium touch emulation and viewport resizing prove browser behavior, not physical iOS/Android rotation or browser chrome behavior. Impact/effort ratings are design judgments. The ten depicted application workflows require product implementation and usability validation before shipping.
