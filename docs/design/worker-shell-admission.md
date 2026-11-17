# Worker shell admission: one grammar, deny by default

Status: revision 3 (2026-11-17) IN REVIEW. Revision 1 was approved by both
supervisors and the operator on 2026-11-12. Revision 2 was rejected on
2026-11-17. Revision 3 answers every defect in that review; see
[Revision 3](#revision-3-2026-11-17).

## Revision 3 (2026-11-17)

Revision 2 (38552c5e16) was rejected with 11 defects by an independent
review: `artifacts/cas-24f2/design-r2-review-2026-11-17.md`. Revision 3
replaces it. Each subsection names the defects it answers (D1–D11).

### R3.1 What went wrong in 5d392, and where else it can happen (D1, D2)

The 5d392 bypass was not a bash disagreement. Tree-sitter-python read a
comment past a lone `\r`, while CPython ended the line there, so the
Python write-API scan checked different code from what CPython ran (cas-5d392
notes). The general lesson: **whenever the guard parses text that another
program will run, the guard's parse must match that program's, or the guard
must refuse.** That applies to bash and to every interpreter whose input the
guard inspects: Python and Node reading a heredoc, `python3 -c`, and
`literal_python_source_edit`.

The review also confirmed a Python hole: with `# -*- coding: utf-7 -*-`,
CPython decodes `+AAo-` as a newline and runs a statement hidden in a comment,
from a file or from stdin.

Places in the interim candidate (`factory/ready-sparrow-54`, c8923e65) where
the guard and the runner can split text differently:

| Location | What it does |
| --- | --- |
| `shell_admission.rs` 688, 949, 1866 | `\r` treated as a line end |
| `shell_admission.rs` 966 | words split on any Unicode whitespace (`char::is_whitespace`) |
| `shell_admission.rs` 1971, `heredoc_delimiters` | `str::lines()` and Unicode whitespace for delimiters |
| `pre_tool.rs` 4414, 4444 | heredoc candidate lines `trim_end_matches('\r')` |
| `shell_effects.rs` 367–368 | same CR trim |

The candidate already refuses CR, VT and non-ASCII whitespace when a heredoc
is present (`shell_effects.rs` 499–503), with tests for CR delimiters
(`pre_tool.rs` ~8937–8945). Revision 2 overstated the flaw. The real gaps
are: those inputs without a heredoc, the hidden-statement and lookalike
shapes in R3.2, and Python/Node bodies.

### R3.2 The fix is structural: one tokenizer owns heredocs (D3)

Listing ambiguous inputs one by one cannot close the class. Revision 3
changes the lexer's structure so it reads the stream the way bash does:

1. **One tokenizer recognises `<<`.** It is the only code that decides
   whether `<<` starts a heredoc. Comments (`# <<X`), arithmetic
   (`$((1<<2))`, `((x<<=1))`), parameter operators (`${v/<<x/y}`) and quoted
   text never start one, because the tokenizer is in the right state when it
   reads them. Every other `<<` scanner is deleted, including
   `heredoc_delimiters`, `check_heredoc_boundaries` and the
   `pre_tool.rs`/`shell_effects.rs` line loops.
2. **Bodies are read in the stream.** At each newline token the tokenizer
   reads the pending heredoc bodies in redirect order, before lexing the next
   line. No pre-pass and no projection removes bodies first.
3. **Line joining follows bash.** Outside single quotes, `\` then newline is
   removed before tokenizing, including inside a `<<` operator (`<\⏎<`) and a
   delimiter word (`EO\⏎F`). In an unquoted-delimiter body, `\⏎` is joined
   before the line is compared with the delimiter, so `x\` then `EOF` is one
   body line and not the end. In a quoted-delimiter body nothing is joined.
4. **Delimiters.** The delimiter is the word after quote removal: plain,
   `'…'`, `"…"`, backslashes and mixtures (`E"O"F`) are admitted. A
   delimiter that is empty, uses `$'…'` or `$"…"`, or contains `$` or a
   backquote is refused (`shell.lexer_ambiguity`).
5. **Exactly one body per `<<`, and one `<<` per body.** A `<<` whose body
   doesn't end on its exact delimiter line, or input left over after the
   last body, is refused. `<<-` strips only leading tabs. A heredoc inside
   `$(…)` whose delimiter line is followed by `)` on the same line (`EOF)`)
   is refused, because bash 3.2 and 5.3 disagree on it (D3e).
6. **Characters.** Only byte `\n` ends a line. Unquoted word boundaries are
   the bash metacharacters: space, tab, newline, `|&;()<>`. These are lexical
   boundaries, not IFS (D8). Any other whitespace is a word character to
   bash. So the tokenizer refuses any non-ASCII whitespace, such as U+00A0,
   rather than guess (D3d): `echo x > W/a⍽/../../out/p` is refused. CR, the
   other C0 controls except tab and newline, and DEL are refused anywhere
   (`shell.lexer_ambiguity`).

### R3.3 Where the check runs (D4, D5)

- **At hook entry**, on the raw command bytes, before any policy or any old
  lexing path. That covers the paths that don't read the tree today:
  `pre_tool.rs` 3661, 4472, 6251 and the credential gate. This entry check
  is the only character check, so no path can skip it.
- **On every decoded payload, at every depth.** Text produced by decoding
  `$'…'` (`shell_admission.rs` 579), a `-c` payload, an `eval` literal and a
  heredoc body fed to an interpreter goes through the same check again.
- **Interpreter bodies.** A heredoc or `-c` body for Python or Node that the
  guard inspects is refused if it has a `coding:` declaration, CR, FF, VT, NUL
  or any non-ASCII character. The scan for writes stays as it is for every
  other body.
- **The shell is recorded, not assumed.** The lexer models bash. For each
  harness the delivery records the shell that actually runs commands, its
  version and any wrapper. Measured here: the Claude harness runs
  `/opt/homebrew/bin/bash` 5.3.9 (`$SHELL`, `$BASH_VERSION`), while
  `/bin/bash` is 3.2. Codex on this Mac and both harnesses on soundwave are
  measured in step 2 of R3.6. If a harness runs a non-bash shell, the grammar
  admits nothing for that harness until it is modelled (question 8).

### R3.4 Rule ids (D6)

- `shell.unterminated` keeps every unterminated quote, substitution,
  arithmetic, escape and compound. The malformed-substitution control keeps
  `shell.unterminated` unchanged.
- `shell.lexer_ambiguity` is only for R3.2/R3.3 inputs: a control or
  non-ASCII whitespace character, an inexact or leftover heredoc, a refused
  delimiter form, and the interpreter-body refusals.
- When both apply, the first by byte offset wins, and each row asserts one id.

### R3.5 Measurement (D9, D10; Q6)

- **Counters ship one release ahead of the grammar**, so the base window uses
  the same counters as the candidate window (Q6). The replay receipts are
  history and the pre-merge regression check; they are not the base rate.
- **Identity without leaking secrets.** Each decision records an HMAC-SHA-256
  of the command bytes, keyed by a per-machine secret stored under the
  operator directory, readable by no agent. It also records the harness, the
  decision, the rule id, the guard version, the session id and an event id.
  These rows are excluded from Cloud sync. No command text is stored.
- **Claims use the event id, not the command.** Every refusal prints its
  event id. A worker reports a suspected false deny with the event id; the
  bug-report path matches on it and never needs the raw command, so nothing
  secret reaches GitHub.
- **Retries counted once.** Decisions are deduplicated per session and HMAC
  before counting.
- **Two rates, both labelled as estimates.** Workers usually rephrase a
  denied command instead of reporting it, so claimed-and-confirmed false
  denies are a lower bound. A rephrase signal is also counted: within the
  same session, a deny followed within 10 minutes by an admitted command with
  the same program word. The report prints, per rule and harness: decisions,
  denies, confirmed false denies (lower bound), and confirmed plus
  rephrase-followed denies (upper bound). The 14-day window starts when the
  grammar release is published. The base is the same report over the 14 days
  before it, on the release with the counters and no grammar.

### R3.6 Integrating the credential-context work: deny if either denies (D11; Q7)

- **cwd resolution** comes from cas-707a2 (types `ShellReadContext`,
  `UNKNOWN_DIRECTORY`, per-statement alternatives) and its tests (Q7).
- **Decisions: deny if either version denies.** Every deny the interim
  candidate has and cas-707a2 lacks is kept and gets a corpus row:
  2eb0fd6 (credential env read from a file into a subprocess,
  sparrow `operator_credentials.rs` 923–954); 233b5f75 (credential reads
  through redirects and xargs argument files); f64f1c40 (unresolved
  directories). cas-707a2's denies are kept unchanged.
- **H-09 on collector error.** The train skips H-09 when
  `shell_effects::collect` errors (`pre_tool.rs` 546–557) and relies on H-29
  to deny. A corpus row pins that the command is still denied.
- **`worker_denied_tools.rs`** changed on both sides. The merge keeps both
  sets of denied tools; its tests from both sides stay.

### R3.7 Proof beyond examples (review: "Is the corpus enough?")

1. **The corpus**: one table-driven test through the real hook, one decision
   and one rule id per row (rows in the corpus note).
2. **A differential test against bash.** For each corpus ALLOW row, wrap the
   command in a function and have the measured bash reprint it
   (`f(){ …; }; declare -f f`). The guard's statement split and heredoc body
   assignment must match bash's reprint. The test runs both bash 3.2 and 5.3
   where present.
3. **Mutation.** For every ALLOW row, insert each R3.2/R3.3 ambiguous input
   (CR, each control, U+00A0, `\⏎`, `# <<X`, `$((1<<2))`, `${v/<<x/y}`, a
   stray `<<`) at every byte offset. Each mutant must either DENY or split
   exactly as bash's reprint does. Failures name the row, the offset and the
   inserted input.

### R3.8 Delivery order

1. This revision reviewed (gating).
2. Measure each harness's shell (R3.3), and ship the counters (R3.5) in the
   next release.
3. Merge the interim candidate onto the train under R3.6, then rebuild the
   tokenizer under R3.2 and add the hook-entry check (R3.3).
4. Corpus, differential and mutation tests (R3.7).
5. The 14-day measurement after the grammar release, reported then. It does
   not block the merge.

### Out of scope, for the operator

`/opt/homebrew` and `~/.local` are writable operator roots that are also on
PATH. A worker could put a binary there that an admitted command name then
runs. The grammar cannot see that; it is a write-roots decision.

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
| `shell.lexer_ambiguity` | A control or non-ASCII whitespace character, an inexact or leftover heredoc, a refused delimiter form, or an interpreter body with a coding line or non-ASCII text (R3.2–R3.4) | The guard could check different text from what bash or the interpreter runs | Remove the character; end each heredoc on its exact delimiter line; write interpreter code to a file |
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

Revision 3 replaces this method with the in-hook counters in R3.5. This
section is the revision 1 record; its receipts remain the pre-merge regression
check for the interim candidates.

The approved measurement is a **fixture-context false-deny rate**, not a
historical-context rate (supervisor decision #4248608, November 14). Deleted
worktrees and grants cannot be reconstructed: the earliest observed occurrence
has a missing cwd for 32,325 of 33,333 Claude commands and 55,430 of 64,432 Codex
commands; another 28 Codex commands have relative cwds. There are multiple cwd
contexts for 48 Claude hashes and 316 Codex hashes. These coverage counts must
accompany the rates. Within each harness, retain the earliest timestamp per
exact command hash and feed the unchanged original command bytes to both
binaries in the same deterministic disposable owned-worktree fixture.

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
  PreToolUse request, to the released base binary (`v4.27.0`, `ff20dd1e`) and
  to the final delivery candidate. Supervisor decision #4248572 on November 14
  pins this baseline; its verified version is `cas 4.27.0 (ff20dd1 2026-11-14)`.
  The earlier `7f6b77429` design reference is superseded. Both binaries receive
  identical environment, cwd shapes and hook inputs; their version lines are
  recorded next to their measurements.
  It records decision and rule.
- **Denominator:** distinct worker Bash commands in the window, deduplicated
  on exact command text.
- **Numerator (false denies):** denied commands that a reviewer judges
  admissible under the policy, meaning read-only or inside the worker's write
  roots, and not operator-only, role, commit-scope or build-cap. The approved
  stratified sample below estimates the numerator; it is not an exhaustive
  classification of every denied command.
- **Output:** fixture-context numerator, denominator and rate at base and at candidate. Each
  candidate deny that the base allowed is listed separately.

### Corrected fixture measurement (final candidate pending)

The frozen observed cohort contains 97,765 distinct harness/command inputs:
33,333 Claude and 64,432 Codex. Supervisor decision #4248985 replaces exhaustive
deny labeling with a fixed-seed, proportional stratified sample of 300 original
BASE denies across 24 harness/rule strata, with at least one row per stratum.
The independent local model labeled 242 TRUE_DENY, 55 FALSE_DENY and three
UNCLEAR. Candidate decisions were excluded from its packet; exact commands and
labels stay local. Sampling seeds are 2402 and 2403 for the operator's random
20-row subset. Operator agreement remains pending.

Supervisor decision #4249274 corrects the fixture to include the repository's
real `.gitignore`, committed and verified byte-for-byte in both linked trees.
Both BASE and the pre-repair `d3a01fc` candidate replayed all 97,765 inputs
successfully in that corrected fixture. Of the 55 original FALSE_DENY labels,
44 ignored-log commands now allow on both binaries; one improves only on the
candidate, and ten still deny. The earlier BASE-only receipt `art-fc999b05`
uses the superseded fixture and cannot supply the corrected-context rate.

The pre-repair candidate added 4,778 denies. A separate blind stratified sample
of 200 across seven harness/rule strata (seed 2404) received 197 FALSE_DENY,
two TRUE_DENY and one UNCLEAR labels after every row was read. The initial
low-confidence label file is excluded. All 244 independently TRUE_DENY inputs
remain denied. The corrected paired aggregate receipt is `art-82f84c5f`:

| Harness | BASE estimated false denies / all inputs | BASE rate | Pre-repair candidate estimate / all inputs | Pre-repair rate |
| --- | --- | --- | --- | --- |
| Claude | 298.071 / 33,333 | 0.89422% | 881.071 / 33,333 | 2.64324% |
| Codex | 411 / 64,432 | 0.63788% | 4,476.598 / 64,432 | 6.94779% |

The pre-repair candidate fails acceptance: it adds an estimated 583 Claude
false denies (simultaneous 95% interval 310–584) and 4,065.598 Codex false denies
(interval 3,834–4,191). Estimates use inverse sampling weights. Intervals invert
finite-population hypergeometric tails, use Bonferroni over 31 strata and four
estimands, cap by full-census eligibility, and retain UNCLEAR as upper bounds.
These intervals describe a fixed candidate and do not account for selecting a
new candidate in response to sampled diagnoses.

The final candidate must show a net reduction in fixture-context false denies
against BASE while retaining every independently TRUE_DENY refusal. Replaying
fixed independent-label frames can measure a changed candidate's contributions;
any denials outside the sampled frames require independent labels before a
total-rate claim. Source inspection repairs admit fixed-prefix glob and quoted
operands while retaining refusals for unquoted expansion that can split argv.
Final candidate replay, operator agreement and integration of the separately
owned credential-context fix remain pending.

The fixture assigns a different worktree from historical workers, so many
originally legitimate writes receive TRUE_DENY labels for leaving its roots.
That biases the historical comparison downward; these figures prove neither a
historical rate nor a formal historical lower bound. The UNCLEAR inputs concern
single-file rustfmt child effects, a candidate binary with unknown effects,
and a process-group signal without ownership evidence. No historical or security
payload executes during any replay.

### Residual new denies the grammar keeps

Supervisor decision #571028 (November 14) accepts a per-class justification
for sampled FALSE_DENY rows whose shape the grammar refuses by design. At
`2eb0fd6` the sampled Claude rows that BASE allowed but the candidate denied
were 16 `shell.dynamic_command` rows and one H-09 row. Literal bindings
(including later and path-prefix uses) and fixed-prefix home globs as find
starting points now resolve or admit. These classes stay refused:

| Class | Why it stays denied |
| --- | --- |
| A value taken from command output (`$(...)`) used as a find starting point or consumer operand | The value exists only at run time. A word such as `-delete` or one that splits into several can become a find action or a consumer option, and no policy can read it first. |
| Unquoted expansion of a multi-word literal value | Word splitting changes argv shape, so the command and option positions the policy reads are not the ones that run. |
| `xargs` feeding a script or a filename reader | Runtime input supplies filenames the credential and write-root policies never see. |
| A glob with no fixed prefix in a consumer option position | A matching filename can start with `-` and select an output, filename source or helper. |
| A syntactically incomplete list | The shell rejects it, so the refusal costs the worker nothing. |

These are the deny-by-default promise. BASE admitted them without seeing
their effects. A sampled row in these classes is reported in its class, not
treated as a defect to admit.

The H-09 row is a fixture artifact. The fixture's synthetic HOME is a sibling
of the worker worktree, so a recursive read of the worker's parent reaches
HOME, which no factory worktree layout allows. The final replay uses a v3
fixture with HOME outside the worker's parent and reruns BASE on that fixture
too. Earlier fixture versions stay immutable.

The v3 fixture mirrors the factory layout. The repository lives under HOME and
the worker is a linked worktree under the repository's `.cas/worktrees`. BASE
replayed all 97,765 inputs on it. Against v2, 50 BASE denies became allows
(32 Claude, 18 Codex), none were added, and no labelled input changed, so the
sampling frames still apply. The interim candidate `6aaa14c` on v3 (receipt
`art-5ab39769`, final candidate pending the credential-context fix):

| Harness | BASE v3 false denies | Candidate v3 false denies | Net change (simultaneous 95% interval) |
| --- | --- | --- | --- |
| Claude | 298.07 | 424.50 | +126.43 (−34 to +217) |
| Codex | 411.00 | 336.74 | −74.26 (−102 to +32) |

All 250 independently TRUE_DENY inputs still deny, including the seven
credential-flow denies, and no candidate deny falls outside the labelled
frames. The H-09 fixture artifact no longer denies. The Claude increase is
the runtime-derived classes above.

### Unresolved directories in credential reads

The credential-context security review (supervisor decision #571215) added six
classes to the corpus. They cover a cd that may fail, an empty or unset cd
target, assignments that never reach the shell and writers that do,
directory-stack targets, wrapped cd, and reads inside quoted substitutions.
Each one must be refused by some rule.

A read whose effective directory is unresolved is checked from every parent
of every operator store. It is also checked from a store itself when an
operand names an entry that exists there. A read operand led by an
unresolved expansion is checked the same way by its literal tail. A cd whose
operand may expand to nothing also reaches HOME. A builtin that writes a
variable (`read`, `declare`, `for`, `unset`, `printf -v` and similar) leaves
that variable unresolved.

One residual stays admitted. When the unresolved directory is a store and
the operand is a bare name that does not exist there, nothing is read from
the store. Refusing that would refuse every relative read after an
unresolved cd.

This rule adds a class to the residual denies above, which decision #571298
of the supervisor accepts without a partial fix. That class is a read from a directory
the guard cannot resolve: a loop variable, a cd to a glob path, or a script
that globs a path passed as an argument. At interim candidate `b622e9e` on the
v3 fixture, seven denies fell outside every labelled frame. A full-census
independent labeller marked all seven FALSE_DENY:

| Harness | BASE v3 false denies | Candidate v3 false denies | Net change (simultaneous 95% interval) |
| --- | --- | --- | --- |
| Claude | 298.07 | 431.50 | +133.43 (−27 to +225) |
| Codex | 411.00 | 336.74 | −74.26 (−102 to +33) |

All 250 independently TRUE_DENY inputs still deny. The operator's random-20
agreement check is not yet exercised.

## Filename-list consumers

This is the admission contract for the consumer migration, not a claim that the
current implementation has completed it. `ALLOW` below always retains existing
credential, execution and write-root policy. Literal file inputs are checked
against their effective cwd; a filename listing grants no permission to read
the files it names. Unknown options or unresolved filenames are refused rather
than assumed to consume inert data. Redirections are independent file inputs
or outputs even when argv consumes only stdin data.

| Consumer | ALLOW modes and reason | DENY modes and reason |
| --- | --- | --- |
| `cd` | Literal directory operand, or resolved HOME/OLDPWD/CDPATH; it changes the cwd for subsequent checks and does not open piped names | Unresolved directory state, because later relative inputs cannot be checked |
| `rg` | Stdin text filtering; literal pattern files (`-f`/`--file`), ignore files (`--ignore-file`) and path operands after credential checks; effective `--files` is a metadata listing | Protected content inputs, including auxiliary config/ignore inputs; unresolved `--pre`/`--hostname-bin` execution. `-l`/`--files-with-matches` and `--files-without-match` read content and receive no listing exemption |
| `find` | Literal starting points and metadata/reference tests, including `-newer`, `-anewer`, `-cnewer`, `-newerXY`, `-samefile`, under existing credential metadata policy; print/print0/printf remain name data | `-files0-from FILE` or stdin when file-list contents cannot be resolved; protected data inputs; `-exec`/`-execdir`/`-ok`/`-okdir` with unknown command or input-derived writer operands; `-delete` or `-fprint`/`-fprint0`/`-fprintf`/`-fls` outside write roots |
| `grep`, `egrep`, `fgrep` | Stdin data; `-f -`/`--file=-` consume pattern data, not filenames; literal positional files, `-f FILE` and `--exclude-from FILE` after credential checks | Protected file/rule inputs and unresolved recursive roots. `-l`/`-L` still read content and receive no listing exemption |
| `head` | Stdin data; literal file operands after credential checks; `-n`/`--lines`, `-c`/`--bytes` are counts | Protected or unresolved file operands; no filename-list input mode is admitted |
| `tail` | Stdin data; literal file operands after credential checks; follow/retry modes keep checking that same path; count/offset options are data | Protected or unresolved file operands; no filename-list input mode is admitted |
| `sort` | Stdin data and literal file operands after credential checks; `-o`/`--output`, `-T`/`--temporary-directory` and TMPDIR under write-root policy; literal `--random-source` under credential policy | `--files0-from FILE`/`--files0-from=-` when the contained filenames are unknown; unresolved `--compress-program` execution; protected random/input files or unauthorized output/temp paths |
| `uniq` | Stdin or first positional input after credential checks; second positional output under write-root policy; `-f`/`--skip-fields` and `-s`/`--skip-chars` are counts | Protected first input, unauthorized second output, or unresolved operands; no filename-list input mode is admitted |
| `wc` | Stdin data and literal file operands after credential checks, including byte-count modes | `--files0-from FILE`/`--files0-from=-` when filenames are unknown; a count does not authorize opening protected inputs |
| `cut` | Stdin data and literal input files after credential checks; selection lists and delimiters are data | Protected or unresolved file inputs; no filename-list input mode is admitted |
| `tr` | Stdin data; operands are character sets, not filenames | Protected input redirections or unauthorized output redirections; there is no argv file-input mode |
| `xargs` | A literal command with checked options; a known data-only or read-only invocation whose input provenance preserves credential policy; `-a`/`--arg-file` is itself a checked file input | Dynamic command position, input-derived writer operands, protected argument files, or an unresolved stream-to-filename content read |

The initial inventory is cas-47e1 artifact `art-d088a676`. On soundwave,
GNU findutils 4.10.0 `find --help` confirms `-files0-from FILE`, reference-file
tests and file-writing/command actions; GNU grep 3.12 `grep --help` confirms
`--exclude-from=FILE`, `-f FILE`, recursion and content-reading `-l`/`-L` modes.
The [GNU coreutils manual](https://www.gnu.org/software/coreutils/manual/)
and [ripgrep argument documentation](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/defs.rs)
define the remaining option contracts. BSD-only or unknown variants require
an explicit parser entry before admission.

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
5. Answered (revision 2 review): keep well-formed heredocs admitted, after
   the R3.2 structural fix.
6. Answered: ship the counters one release ahead of the grammar.
7. Answered: cas-707a2 for cwd resolution; deny if either version denies.
8. (Revision 3) If a harness runs commands under a shell other than bash,
   should that harness be forced to bash, or should the guard admit nothing
   there until that shell is modelled?
9. (Revision 3) The per-machine HMAC key lives under the operator directory.
   Is that the right home, and should rotating it be an operator command?

## Reviews

| Reviewer | Verdict | Date | Notes |
| --- | --- | --- | --- |
| Supervisor (prowl, quick-hawk-35) | APPROVE | 2026-11-12 2:49 PM ET | 1: keep literal scripts admitted as opaque effect units; the write-root check still applies to the path. 2: refuse coproc outright. 3: depth 6 accepted; report the depth distribution from the replay, and raise it before merge if a real command needs more. 4: label rule accepted; the rule-based reviewer labels every deny and the operator spot-checks a random 20 rows. |
| Supervisor (soundwave) | APPROVE | 2026-11-12 2:50 PM ET | Q1 keep literal scripts opaque: yes. Q2 refuse coproc: yes. Q3 depth 6: yes. Q4 rule-labelled false denies with operator spot-check: yes. Required changes (prowl accepts both): A, the measurement includes Codex session logs and reports per harness; B, replay receipts never copy raw secret-shaped command text. Both are folded into the measurement section. |
| Operator | APPROVE | 2026-11-12 2:51 PM ET | Approved with soundwave's changes A and B (recorded on cas-24f2 by the prowl supervisor). |
| Independent review (revision 2) | REJECT | 2026-11-17 ~9:30 AM ET | 11 defects; `artifacts/cas-24f2/design-r2-review-2026-11-17.md`. Answered in revision 3. |
| Supervisor (prowl) | PENDING (revision 3) | | |
| Supervisor (soundwave) | PENDING (revision 3) | | |
| Operator | PENDING (revision 3) | | |
