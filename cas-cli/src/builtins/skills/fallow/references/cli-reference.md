# Fallow CLI Reference

This skill no longer vendors a copy of the CLI reference. The copy that shipped
here was from the 2.57 era (config schema 3) and missed whole commands, so
read the reference from the installed binary instead. It always matches the
version you are running.

| You need | Run |
|----------|-----|
| Every command, flag, exit code, issue type, output format, environment variable, MCP tool and plugin, as JSON | `fallow schema` |
| One command's flags and defaults | `fallow <command> --help` |
| What one issue type means and how to fix it | `fallow explain <issue-type> --format json` |
| The config file JSON Schema | `fallow config-schema` |
| The external plugin JSON Schema | `fallow plugin-schema` |
| Which config file is in effect | `fallow config --path` |

Useful `fallow schema` keys: `commands`, `global_flags`, `exit_codes`,
`issue_types`, `suppression_comments`, `output_formats`,
`environment_variables`, `mcp_tools`, `plugins`.

Run these like any other fallow command: JSON on stdout, stderr discarded, and
the exit status printed (`2>/dev/null; echo "exit=$?"`). Full documentation:
<https://docs.fallow.tools>.
