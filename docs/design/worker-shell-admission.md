# Worker shell admission: one grammar, deny by default

Status: DRAFT for review (cas-24f2). Implementation waits for reviews from both
supervisors and the operator. Reviewers record a verdict in the Reviews section
at the end.

## Problem

The worker Bash PreToolUse hook makes 11 policy decisions from shell text. Each
decision is a deny-list over its own partial parse. A construct a parser does not
model is therefore allowed unless it happens to trip a "cannot resolve" rule.
Adversarial rounds on 10/30 found a new class each time: stdin interpreters,
coproc, trap actions, PS4 with xtrace, alias bodies, and escaped backticks
(cas-dc36 notes). The operator's 10/30 decision ("guard redesign after") made
this redesign the next guard work.

These numbers come from the current tree (`pre_tool.rs` and
`pre_tool/shell_effects.rs`, with the cas-dc36 scanner at 6a6acdbce).

- About 15 lexing routines. The closing-backtick scan is written 5 times
  (pre_tool.rs:3916, 6420, 6475; shell_effects.rs:346, 425). Quote and escape
  rules are re-implemented in `heredoc_delimiters`,
  `check_heredoc_boundaries` and `collect_shell_substitutions`.
- Recursion limits disagree, and two of them fail open:
  - `collect_shell_invocations` (>3) and `role_env_tampering_at_depth` (>3) stop
    checking and return silently;
  - the Rust build guard checks fewer than 2 levels;
  - `collect_git_write_targets` (>6) and `collect_script` (>8) fail closed.
- Unchecked today: commands a bare `bash`/`sh -s`/`source /dev/stdin` reads
  from a heredoc, here-string or pipe; trap action strings; PS4 under `set -x`;
  alias bodies; xargs arguments that arrive on stdin.
- False denies go the other way. Read-only work is refused with generic text
  that never names the construct: #1866 (`$((missing+1))`), #1814 (a Markdown
  heredoc treated as a nested shell), #1812/#1816/#1836 (`bash -n`), and
  #1815/#1820/#1822/#1823/#1806 (literal scripts). While surveying this note,
  a read-only `sed -n "$((S+1)),$((S+2))p"` was refused as "an unresolved
  arithmetic operand".

## Proposal

### One lexer and parser, one tree

A new module `pre_tool/shell_admission.rs` owns quoting, escapes, heredocs,
substitutions and recursion. It produces a `Script` tree or an `Unsupported`
refusal. Every guard reads the tree; none re-lexes text. The duplicate lexers
are deleted in the same change, so there is no parallel path. That covers
`factory_shell_lex`, the substitution, heredoc and backtick scanners in both
files, and `attribution::split_shell_statements` where the guard uses it.
cas-dc36's shared scanner is the starting point and is extended, not forked.

### Grammar: what is admitted

Anything outside this list is refused, with an explanation (next section).

1. **Words:**
   - literals; `'…'`; `"…"` containing only admitted expansions; `$'…'`;
   - `$VAR` / `${VAR}`, plus `${VAR:-literal}` and the other default forms
     whose operand is admitted;
   - tilde, and globs (treated as data).
2. **Simple commands:** an optional `NAME=word` prefix, a command word, then
   arguments.
   - The command word must be a literal after unwrapping known wrappers (`env`,
     `command`, `nice`, `timeout`, `time`, `nohup`, `xargs` with a literal
     command).
   - A non-literal command word (`$cmd args`) is refused.
3. **Lists and grouping:**
   - pipelines `|` and `|&`, and lists `;`, `&&`, `||`, `&`, newline;
   - `( … )` and `{ …; }`;
   - `if`, `for`, `while`, `until` and `case`, with every branch parsed as if
     it runs.
4. **Redirects:**
   - `<`, `>`, `>>`, `&>`, `2>&1` and `N>&M` onto literal or admitted-word
     targets;
   - `{fd}>` with a literal target.
5. **Substitutions:** `$( … )`, `` `…` `` and `<( … )` / `>( … )`. The body is
   parsed recursively under this grammar, and its commands count as executing.
6. **Arithmetic:** `$(( … ))` and `(( … ))` whose operands are integer
   literals, names or admitted expansions. Assigning to a name is local state
   with no filesystem effect (#1866).
7. **Heredocs:**
   - Quoted delimiter (`<<'EOF'`, `<<"EOF"`): the body is data for the
     receiving command.
   - Unquoted delimiter: the body is scanned for substitutions only, and each
     one is parsed as rule 5.
   - The receiving command decides what the body means; see rule 8.
8. **Interpreters, by input source:**
   - A literal script path (`bash x.sh`, `python3 -I tests/x.py`, `sh -n
     x.sh`) is admitted. The script file is the effect unit, which is the
     existing workspace contract (#1806, #1815, #1816, #1820, #1822, #1823,
     #1836).
   - `-c 'literal'` parses the payload recursively.
   - Python or Node reading a quoted heredoc (`python3 - <<'EOF'`) keeps the
     existing write-API scan of the body (cas-17a5).
9. **eval:** admitted only with a literal payload, which is parsed recursively.
10. **Function definitions:** the body is parsed as if it runs where the
    function is defined.
11. **Depth:** one limit (proposed 6) for every recursion. Exceeding it is an
    explained refusal, never a silent stop.

### Refused constructs, each with its reason

Each refusal has a stable rule id plus the byte offset and snippet. Tests
assert the rule id, never the prose.

| Rule id | Construct | Why it cannot be checked | Supported alternative |
| --- | --- | --- | --- |
| `shell.stdin_interpreter` | A shell or interpreter reading commands from a heredoc, here-string, pipe or `/dev/stdin`, with no script and no `-c` (`bash <<EOF`, `sh -s`, `echo … \| bash`, `source /dev/stdin`, `source <(…)`) | The program text is input, not an argument | Write the script to a file in the worktree and run it, or use `bash -c '…'` |
| `shell.deferred_code` | `trap` with an action, PS4/PROMPT_COMMAND assignment, `alias`, `shopt -s expand_aliases` | The string runs later as code | Run the command directly |
| `shell.dynamic_eval` | eval, `bash -c` or `source` whose payload is not literal | The payload is unknown until run time | Use a literal payload |
| `shell.dynamic_command` | A command word or xargs command that is not a literal | The program to run is unknown | Name the program |
| `shell.coproc` | `coproc` | Rarely needed; its two forms are ambiguous | Use a pipeline or a background job |
| `shell.unterminated` | An unterminated quote, substitution or heredoc | The parse is incomplete | Close the construct |
| `shell.depth` | Nesting deeper than 6 | Bounded analysis | Flatten the command or use a script file |
| `shell.unknown_syntax` | Anything else the parser does not recognize | Deny by default | Named in the refusal text |

The refusal text is
`SHELL CONSTRUCT REFUSED [<rule id>] at byte <n> (<snippet>): <why>. Instead: <alternative>.`

It replaces the generic "cannot resolve shell filesystem effects" and "unknown
nested shell scope" wording wherever the cause is grammatical.

### Policy layer: unchanged rules, one input

The 11 decisions keep their current policy and order. Each reads the tree, not
the text:

1. operator-only commands;
2. role-variable tampering;
3. supervisor-only MCP commands;
4. config-file writes;
5. push hook bypass;
6. kill patterns;
7. dangerous formatters;
8. Rust build caps;
9. commit/push scope;
10. QA merge;
11. workspace write roots.

The credential-path checks (#1808, #1838, #1754) read the same tree. This task
adds no new policy, monitor or check; any proposal for one goes to the
supervisor first.

## Acceptance corpus

The corpus is a single table-driven test, `shell_admission_corpus_cas_24f2`. It
replays each row as a real PreToolUse JSON request through
`handle_pre_tool_use` with role=worker and asserts the decision and the rule id.
Its output, row by row, is the inspectable receipt.

Rows:

- **Repro rows from art-64dbfb55:**
  - #1866 and #1838, which must ALLOW, or #1838 must name the actual protected
    subroot;
  - #1808 and #1754 (ALLOW, no credential read);
  - #1814 (ALLOW, heredoc as data);
  - #1812/#1816/#1836 `bash -n` and #1806/#1815/#1820/#1822/#1823 literal
    scripts (ALLOW);
  - #1752/#1603/#1780 scratch and symlink effects (ALLOW);
  - #1615/#1677/#1856/#1860 branch-state messages (ALLOW, with the existing
    distinctions);
  - #1770 nested `bash -c` commit outside the worktree (DENY).
- **cas-17a5:** a quoted Python heredoc (ALLOW).
- **The 10/30 bypass classes (DENY, each with its rule id):** stdin
  interpreters, coproc, trap, PS4, alias, the escaped backtick and the `\"`
  inside a quoted backtick, plus `xargs` reading stdin into a writer.
- **Controls:** every existing ALLOW and DENY row in `cas_3147_*` and
  `nested_shell_commit_scope_cas_4923e` stays in its current test, unchanged.
  A row that changes decision is listed in the delivery with its reason.
- **Out of scope:** #1700 stays with cas-0204.

## False-deny measurement (14 days, base and candidate)

- **Source:**
  - Claude harness transcripts under `~/.cas/claude-base/projects/*/*.jsonl`
    (915 files changed in the last 14 days);
  - Codex session logs under `~/.codex*/sessions`.

  Each worker shell command is one input. No guard log stores command text, so
  these logs are the only full record. Results are reported per harness.
- **Secrets:** receipts never copy raw command text that may hold secrets.
  Tokens and env values are redacted. Any secret-shaped command keeps only a
  SHA-256 hash with its decision and rule.
- **Replay:** a one-off script (not shipped) feeds each command, as a worker
  PreToolUse request, to the base binary (`7f6b77429`) and to the candidate.
  It records decision and rule.
- **Denominator:** distinct worker Bash commands in the window, deduplicated
  on exact command text.
- **Numerator (false denies):** denied commands that a reviewer judges
  admissible under the policy, meaning read-only or inside the worker's write
  roots, and not operator-only, role, commit-scope or build-cap. Every deny is
  listed in a labeled CSV, together with the labeling rule. That CSV is the
  receipt.
- **Output:** numerator, denominator and rate at base and at candidate. Each
  candidate deny that the base allowed is listed separately.

## Delivery plan

1. This note, then reviews from both supervisors and the operator (gating).
2. `shell_admission.rs` (lexer, parser, tree and refusals) and the corpus
   test. Guards are ported one by one onto the tree, and duplicate lexers are
   deleted as each guard moves.
3. The measurement, with its receipts under the task's artifacts directory.

## Questions for reviewers

1. Should literal script paths stay admitted as opaque effect units (today's
   ALLOW controls)? If not, the grammar must refuse them and the controls
   change.
2. Is `coproc` refused outright, or admitted with both of its forms parsed?
3. Is a depth limit of 6 acceptable?
4. Is the false-deny label rule above acceptable, or should every deny be
   labeled by the operator?

## Reviews

| Reviewer | Verdict | Date | Notes |
| --- | --- | --- | --- |
| Supervisor (prowl, quick-hawk-35) | APPROVE | 2026-11-12 2:49 PM ET | 1: keep literal scripts admitted as opaque effect units; the write-root check still applies to the path. 2: refuse coproc outright. 3: depth 6 accepted; report the depth distribution from the replay, and raise it before merge if a real command needs more. 4: label rule accepted; the rule-based reviewer labels every deny and the operator spot-checks a random 20 rows. |
| Supervisor (soundwave) | APPROVE | 2026-11-12 2:50 PM ET | Q1 keep literal scripts opaque: yes. Q2 refuse coproc: yes. Q3 depth 6: yes. Q4 rule-labelled false denies with operator spot-check: yes. Required changes (prowl accepts both): A, the measurement includes Codex session logs and reports per harness; B, replay receipts never copy raw secret-shaped command text. Both are folded into the measurement section. |
| Operator | pending | | |
