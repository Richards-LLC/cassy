# Commander responsive fixture regressions

The cas-12c29 lane runs the eleven HUB-J1/J2/J3/J5/J7/J8/J10/J11 titles whose original fixtures assumed desktop controls at 390px. It serves the production bundle through the hub protocol double; it does not prove a native Android browser or real daemon.

From `hub-web`, run each variant separately with fresh receipt/output directories:

```bash
JOURNEY_LAYOUT=phone JOURNEY_SCHEME=light npm run journeys:responsive -- --workers=1
JOURNEY_LAYOUT=phone JOURNEY_SCHEME=dark npm run journeys:responsive -- --workers=1
JOURNEY_LAYOUT=desktop JOURNEY_SCHEME=light npm run journeys:responsive -- --workers=1
JOURNEY_LAYOUT=desktop JOURNEY_SCHEME=dark npm run journeys:responsive -- --workers=1
```

Defaults are phone/light. Each invocation selects exactly eleven tests. Set `JOURNEY_OUTPUT` and `JOURNEY_RECEIPTS` to distinct durable paths when retaining all four variants; the fixture's default output directory is cleared between runs. In factory sessions register the dist server first and use a config overlay with `webServer` disabled, following cas-servers.

Desktop scenarios retain their existing assertions. Phone flows use the actual product breakpoint, touch activation, list → conversation → Back navigation, and the phone's hint-free search. Goal completion checks real sends/reply correlation, exact target machine/ask ID, retained draft/caret, answered question retirement, dark-theme persistence and visible phone controls. Clock-age checks return to the visible list instead of querying hidden sidebar roles. A responsive outage stage returns to the invoking viewport rather than forcing desktop.

`responsive-goals.ts` uses explicit declared choices. It opens the baseline's folded ask through its visible expand control, while allowing the polished one-in-flow ask to expose the same choice directly. Missing controls or failed goals still fail; no test is skipped, no viewport is enlarged to make a phone case pass, and no product behavior is mocked beyond the existing hub protocol double.
