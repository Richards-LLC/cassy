# `cas hub service status` launch-readiness brief

- **First two lines:** The service state stays first, followed by its log location; unavailable CLI findings then show the cause and the next command.
- **Scannable:** Ready CLIs use one short row; unavailable profiles use a summary, a home-relative path, and one repair command.
- **Readable:** Profile paths use `~` when they are under the home directory; repair commands name the affected CLI and profile.
- **Machine output:** `--json` remains one `ServiceReport` object with `launch_readiness[]`; each `detail` keeps its complete error text and absolute path.
- **Omitted:** Ready executable paths and repeated error prose are omitted from the human scan and remain available in JSON.

## Critique

Pending strict terminal QA at 80 and 120 columns.
