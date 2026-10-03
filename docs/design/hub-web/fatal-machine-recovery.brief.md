# Stopped machine connection

Reader: An operator whose connected machine has stopped because this browser cannot run the required connection API.
Decision: Update to a supported browser and reload, rather than wait for a retry that cannot fix it.
Hero form: Retain the last session frame with its existing connection banner, and one machine Attention entry with the same reason and recovery.
Distinctive move: Explain the missing browser feature in plain words, keep the actual compatible browser versions from the transport reason, and state reload as the next step.
Omitted: New layout, retry semantics, pairing and session-only outage vocabulary; existing Pebble surfaces and tokens stay in use.

The machine's fatal verdict outranks a stale attach retry. Tasks retain last-state age but cannot promise reconnect. Existing footer machine-state precedence is explicitly pinned; duplicate session transport notice is resolved under the machine notice. Full technical reason remains behind Details.

## Critique

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Existing Pebble retained-frame banner and machine Attention entry use the product's machine names and connection vocabulary. |
| Fit | 4 | The operator receives the specific browser compatibility recommendation and reload step, rather than a network retry that cannot fix the browser. |
| Hierarchy | 4 | The banner leads with the lost machine, then the missing browser feature, then the supported browser versions and reload step. |
| Craft | 4 | Recovery wraps at 390, status keeps its last-state age, and one machine notice replaces the duplicate session notice. |
| Accessibility | 4 | Live-region text changes once; Tasks and paired-machine state are keyboard-reachable, and three media modes have explicit query assertions. |

Production previews are checked at 1280/light and 390/dark, including phone Tasks and keyboard Paired machines. The control explanation requires browser recovery too. The ordinary retry, pairing and session-only paths are preserved; the retry return, pairing and keyboard journeys run separately. The strict fatal-browser stimulus supplements real production cells; production JavaScript-disabled findings match the base. Actual browser installation is not claimed: recovery reload restores the required API in the test browser.
