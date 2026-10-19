# `cas hub service install --dry-run`

**First two lines:** The first line confirms the launchd or systemd preview changes nothing; the next lines say which service file would be written and which manager steps would run.

**Scannable:** A short plan lists the user service file and the manager actions without dumping XML or long command paths.

**Readable:** The exact service definition and manager commands appear only when the operator adds `--full`.

**Machine output:** `--json` remains one object with `dry_run`, `platform`, `supervision`, `unit_path`, `log_path`, `actions`, and `definition`; no human summary appears in JSON.

**Omitted:** The plist or systemd unit body is omitted from the default human preview and is available with `--full`.
