# `cas hub service install --dry-run`

**First two lines:** The first line confirms the launchd or systemd preview changes nothing; the next lines say which service file would be written and which manager steps would run.

**Scannable:** A short plan lists the user service file and the manager actions without dumping XML or long command paths.

**Readable:** The exact service definition and manager commands appear only when the operator adds `--full`.

**Machine output:** `--json` remains one object with `dry_run`, `platform`, `supervision`, `unit_path`, `log_path`, `actions`, and `definition`; no human summary appears in JSON.

**Omitted:** The plist or systemd unit body is omitted from the default human preview and is available with `--full`.

## Critique

terminal-qa: PASS cas hub service install --dry-run · 12 runs · 0 fail · 0 warn · 0 allowed · &lt;artifacts&gt;/cas-0051/terminal-qa/after/report.json

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Line 1 says the dry run changed no files or manager state; line 2 names the proposed service path, and line 4 gives one `--full` command. |
| Fit | 5 | The four-line human preview groups the manager actions into one scannable `Would run` line; the plist appears only after `--full`. |
| Craft | 5 | All default captures fit within 72 cells at 80 and 120 columns with no wraps. |
| Theme safety | 5 | Four palettes pass; the C-locale capture uses an ASCII hyphen and `NO_COLOR` remains readable. |
| Machine contract | 5 | The JSON capture is one object with the seven documented fields and empty stderr. |

Scored by golden-phoenix-74 on 2026-10-19; floor holds.
