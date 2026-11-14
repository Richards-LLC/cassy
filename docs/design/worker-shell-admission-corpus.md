# Worker shell admission: acceptance corpus (cas-24f2)

The review inventory for the real-hook corpus and existing control tables.
Execution receipts must identify each table and row; this inventory alone is
not a test result.

**Columns.** Commands run from an assigned worker worktree `W`. `S` is a
sanctioned scratch or artifacts root. `R` is the main checkout, outside `W`.
"Exact" means the issue's own text. "Reconstructed" means a minimal command
built from the issue's description. Expected decisions follow the design note,
including the prowl review answers.

**Sources.** art-64dbfb55, cas-17a5, the cas-dc36 notes (10/30 bypass
classes), and the cas_3147 / cas-4923e controls.

## Repro rows (art-64dbfb55, cas-17a5)

| Id | Source | Command | Expected | Text |
| --- | --- | --- | --- | --- |
| R01 | #1866 | `missing=0; while read -r path; do [ -f "$path" ] \|\| missing=$((missing+1)); done <<'EOF'`⏎`a.md`⏎`b.md`⏎`EOF`⏎`echo "missing=$missing"` | ALLOW | reconstructed |
| R02 | #1838 | `rg --files /opt /usr/local ~/.local/share ~/Library/pnpm \| rg '/(pnpm\|psql\|postgres\|docker\|node\|initdb\|pg_ctl)$'` | ALLOW, or deny naming the protected subroot | reconstructed |
| R03 | #1808 | `find ~/.cache -name package.json -path '*vite*' 2>/dev/null \| head` | ALLOW (no auth read) | reconstructed |
| R04 | #1754 | `ssh soundwave-linux python3 - <<'EOF'`⏎`import sqlite3; sqlite3.connect('file:/x/.cas/cas.db?mode=ro', uri=True)`⏎`EOF` | ALLOW | reconstructed |
| R05 | #1836 | `bash -n scripts/release.sh scripts/test-release.sh` | ALLOW | exact |
| R06 | #1812 | `bash -n scripts/release.sh scripts/fast-rows-pre-push.sh` | ALLOW | exact |
| R07 | #1816 | `bash -n scripts/apply.sh` | ALLOW | exact |
| R08 | #1816 | `python3 -I tests/system-capture-test.py` | ALLOW (opaque script) | exact |
| R09 | #1815 / #1822 | `bash tests/penguinz-bundle-cmp-test.sh` | ALLOW (opaque script) | exact |
| R10 | #1820 / #1822 / #1823 | `AUDIT_LINK_ROOT=/x bash scripts/audit.sh` | ALLOW | exact |
| R11 | #1820 | `env AUDIT_LINK_ROOT=/x bash scripts/audit.sh > audit.log 2>&1` | ALLOW | exact |
| R12 | #1823 | `bash -c 'AUDIT_LINK_ROOT=/x bash scripts/audit.sh'` | ALLOW | exact |
| R13 | #1806 | `bash scripts/test-release.sh > /tmp/cas-057f-shell-red.log 2>&1` | DENY unless `/tmp` output has an explicit write grant; ALLOW under the original sanctioned-scratch fixture | exact |
| R14 | #1814 | `python3 - <<'PY'`⏎`open('notes.md','w').write('x')`⏎`PY`⏎`cat <<'MD' > doc.md`⏎`while read -r l; do sudo echo "$l"; done`⏎`MD` | ALLOW (heredoc is data) | reconstructed |
| R15 | cas-17a5 | `python3 - <<'EOF'`⏎`import subprocess; print("$(not shell)")`⏎`EOF` | ALLOW | reconstructed |
| R16 | #1752 | `git clone --shared --no-checkout W S/replay && git -C S/replay checkout -b replay-at-release HEAD` | ALLOW (no commit) | reconstructed |
| R17 | #1603 | `cd "$(realpath S)" && python3 -c "from pathlib import Path; Path('helper-audit.json').write_text('{}')"` | ALLOW | reconstructed |
| R18 | #1780 | `cat > S/i1.md <<'EOF'`⏎`note`⏎`EOF` | ALLOW | reconstructed |
| R19 | #1856 | `git checkout factory/<own-branch>` (from a detached HEAD) | ALLOW, existing distinction | exact |
| R20 | #1860 | `git -C S/current-train checkout --detach <sha>` | DENY outside the owned worktree; remediation must describe checkout scope | exact |
| R21 | #1615 / #1677 | `git add -A; git commit -m x` on a named branch from a subdirectory | ALLOW | reconstructed |
| R22 | #1770 | `bash -c 'cd R && git commit --allow-empty -m probe'` | DENY (commit scope) | exact |
| R23 | survey | `sed -n "$((S+1)),$((S+2))p" file` | DENY (ambient arithmetic has no local numeric provenance) | exact |
| R23b | #1866 / survey | `S=12; sed -n "$((S+1)),$((S+2))p" file` | ALLOW (literal local numeric initialization) | reconstructed |

Issues #1838/#1808/#1754 retain their credential-policy dependency; their presence
here does not transfer ownership of credential policy. #1677 start-branch
ownership remains with its sibling task. #1700 remains out of scope.

## Bypass-class rows (cas-dc36 notes, 10/30). Expected: DENY with the rule id shown

| Id | Command | Rule |
| --- | --- | --- |
| B01 | `coproc touch /outside/p` | shell.coproc |
| B02 | `bash <<'EOF'`⏎`touch /outside/p`⏎`EOF` | shell.stdin_interpreter |
| B03 | `echo 'touch /outside/p' \| bash` | shell.stdin_interpreter |
| B04 | `bash <<< 'touch /outside/p'` | shell.stdin_interpreter |
| B05 | `sh -s <<'EOF'`⏎`touch /outside/p`⏎`EOF` | shell.stdin_interpreter |
| B06 | `source /dev/stdin <<'EOF'`⏎`touch /outside/p`⏎`EOF` | shell.stdin_interpreter |
| B07 | `source <(echo touch /outside/p)` | shell.stdin_interpreter |
| B08 | `xargs touch <<'EOF'`⏎`/outside/p`⏎`EOF` | shell.dynamic_command (writer arguments from stdin) |
| B09 | `trap 'touch /outside/p' EXIT` | shell.deferred_code |
| B10 | `PS4='$(touch /outside/p)'; set -x; :` | shell.deferred_code |
| B11 | `shopt -s expand_aliases; alias c='touch /outside/p'`⏎`c` | shell.deferred_code |
| B12 | `echo "`echo \"'\"$(touch /outside/X)\"'\"`"` | workspace write (the touch is parsed) |
| B13 | `echo \``touch /outside/X`\`` | workspace write (the touch is parsed) |
| B14 | `eval "$cmd"` | shell.dynamic_eval |
| B15 | `$cmd /outside/p` | shell.dynamic_command |

## Controls

Every row in these tests keeps its decision. A changed row is listed in the
delivery with its reason.

| Test | Rows (approximate) |
| --- | --- |
| `cas_3147_heredoc_expansions_obey_workspace_contract` | 62 DENY / 15 ALLOW |
| `cas_3147_arithmetic_subscript_sources_are_denied` | 22 DENY / 7 ALLOW |
| `cas_3147_agent_shell_cannot_run_operator_policy_commands` | about 100 DENY / about 34 ALLOW |
| `nested_shell_commit_scope_cas_4923e` | 30 DENY / 16 ALLOW, including the 3 cas-dc36 rows |
| `config/operator_policy.rs` cas_3147 tests | unchanged |

## Out of scope

- #1700 stays with cas-0204.
