# Worker shell admission: one grammar, deny by default

Status: revision 2 (2026-11-17) IN REVIEW. Revision 1 was approved by both
supervisors and the operator on 2026-11-12; see Reviews. Revision 2 changes
three things, all listed in [Revision 2](#revision-2-2026-11-17): a lexer-agreement
rule that applies before any ALLOW, the measurement method, and the integration
plan for the credential-context work that landed separately.

## Revision 2 (2026-11-17)

### R2.1 Lexer agreement before any ALLOW

The 5d392 bypass (soundwave, November 17) used a carriage return: the guard's
lexer read a line break where bash reads an ordinary character. The worker
then ran something other than what the guard had checked. The same flaw is in
the interim candidate. `shell_admission.rs` on `factory/ready-sparrow-54`
(c8923e65) treats `\r` as a line end at lines 688, 949 and 1866. It finds
heredoc delimiters with `str::lines()` at line 1971, which also splits on
`\r\n`. Bash splits only on `\n`, so `EOF\r` does not end a heredoc
opened with `EOF`.

Rule: a parse-based ALLOW is valid only when the lexer and bash must read the
text the same way. When they could disagree, the command is refused before
any policy runs, under one new rule id, `shell.lexer_ambiguity`. It applies
to every ALLOW path that reads the tree, including all eleven policy decisions
and the credential checks. These inputs are refused:

| Input | Why the lexer could disagree with bash |
| --- | --- |
| A carriage return (`\r`) anywhere | Bash treats it as a word character, not a line end |
| Any other C0 control except tab and newline, and DEL (0x7F) | Terminal, locale and builtin handling differ (NUL ends a C string; `\v`/`\f` are not IFS by default) |
| An unterminated single, double or `$'…'` quote, backquote, `$(`, `$((`, `${` or escape at end of input | Bash would read more input or fail; the guard cannot know which |
| A heredoc that does not end on a line exactly equal to its delimiter (after tab removal for `<<-`), or that runs to end of input | Bash reads to end of input; the guard's body is not bash's body |
| A heredoc delimiter that is partly quoted, contains an expansion or is empty | Bash's delimiter rules (quote removal, no expansion) are easy to get wrong |
| Two or more heredocs on one line whose bodies the lexer cannot assign in order | Bash reads the bodies in redirect order |

Implementation rules that keep the lexer bash-exact:

1. Only byte `\n` ends a line. `str::lines()`, `split_whitespace()` and other
   Unicode-aware splitting are not used in the lexer or the heredoc reader.
2. IFS is space, tab and newline only.
3. The lexer runs on the exact bytes the harness passes to bash. Nothing is
   trimmed or normalized first.
4. `shell.lexer_ambiguity` wins over every other rule id. The refusal names
   the byte offset and the character class, never the raw character.

Well-formed heredocs stay admitted (#1866, #1814, cas-17a5 and R18 must still
ALLOW). Only the ambiguous shapes above are refused. Reviewer question 5 asks
whether all heredocs should be refused instead.

Each input class above is a corpus row (rows L01–L09 in the corpus note),
paired with the same command in its well-formed spelling as an ALLOW row.

### R2.2 Measurement: wired in, measured after publication

Revision 1 measured the 14-day rate by replaying old transcripts in a
synthetic worktree. That measured a fixture, not workers. Missing cwds,
deleted worktrees and grants made it a fixture-context rate only (see the
replaced section below). Revision 2 replaces it with counters in the hook. The
replay receipts below stay as history and as the pre-merge regression check.
They are no longer the acceptance rate.

1. **Counted at the decision.** Every worker Bash PreToolUse decision adds
   one row to the local quality-event ledger: harness, decision (allow or
   deny), the rule id for a deny, the guard version and a SHA-256 of the
   command bytes. No command text is stored.
2. **False denies are labelled, never inferred.**
   - A worker that reports a refusal with `system action=report_cas_bug`
     carries the rule id as the leading gate code. The report is matched to
     the deny event by command hash and counts as a claimed false deny.
   - A supervisor or the operator confirms or rejects the claim with one
     labelling action, which records the verdict and who gave it.
   - Unclaimed denies count as true denies. Claimed but unlabelled denies are
     reported separately as an upper bound.
3. **Rate.** For each rule and harness: confirmed false denies, total
   decisions (the denominator), and the rate, plus the unlabelled upper bound.
   One report command prints it for any window.
4. **Window.** The 14 days start when the release with the grammar is
   published. The base rate is the same counters from the last release
   without the grammar, over the 14 days before publication. The wiring ships
   one release ahead of the grammar, or in the same release if that is
   approved (reviewer question 6).

### R2.3 Integrating the credential-context work

cas-707a2 reworked credential-read cwd tracking on the release train
(`shell_read_contexts` returning `Option`, `UNKNOWN_DIRECTORY`, per-statement
cwd alternatives). The interim candidate rewrote the same code independently.
A trial merge of `factory/ready-sparrow-54` into the train tip conflicts in
`operator_credentials.rs` (2 hunks), `pre_tool.rs` (1) and
`pre_tool/shell_effects.rs` (8).

Plan: cas-707a2's behaviour is authoritative, because it is merged and
reviewed. The merge keeps its read-context types and refusals and feeds them
from the admission tree instead of a second lexer. Every cas-707a2 test stays
unchanged. Where the two implementations decide differently, the cas-707a2
decision is kept and the difference is listed in the delivery.

### R2.4 Delivery order (replaces the revision 1 plan)

1. This revision reviewed (gating, as before).
2. Merge the interim candidate onto the train with R2.3, and add R2.1 to the
   lexer.
3. One table-driven corpus test through the real hook, covering every repro,
   bypass, lexer-ambiguity and control row. It names the rule id and asserts
   the decision and rule id per row.
4. The measurement wiring (R2.2). The 14-day measurement runs after
   publication and is reported then; it does not block the merge.

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
| `shell.lexer_ambiguity` | A carriage return, another control character, or a heredoc or quote shape bash could read differently (R2.1) | The guard could check different text from what bash runs | Remove the control character; end each heredoc on its exact delimiter line |
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

Revision 2 replaces this method with the in-hook counters in R2.2. This
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
5. (Revision 2) R2.1 keeps well-formed heredocs admitted and refuses only the
   ambiguous shapes. Should all heredocs be refused instead? That would turn
   #1866, #1814, cas-17a5 and R18 into denies.
6. (Revision 2) Should the measurement wiring ship one release before the
   grammar, so the base rate is measured live, or in the same release, with
   the base taken from the replay receipts?
7. (Revision 2) Is cas-707a2 authoritative where it and the interim candidate
   disagree (R2.3)?

## Reviews

| Reviewer | Verdict | Date | Notes |
| --- | --- | --- | --- |
| Supervisor (prowl, quick-hawk-35) | APPROVE | 2026-11-12 2:49 PM ET | 1: keep literal scripts admitted as opaque effect units; the write-root check still applies to the path. 2: refuse coproc outright. 3: depth 6 accepted; report the depth distribution from the replay, and raise it before merge if a real command needs more. 4: label rule accepted; the rule-based reviewer labels every deny and the operator spot-checks a random 20 rows. |
| Supervisor (soundwave) | APPROVE | 2026-11-12 2:50 PM ET | Q1 keep literal scripts opaque: yes. Q2 refuse coproc: yes. Q3 depth 6: yes. Q4 rule-labelled false denies with operator spot-check: yes. Required changes (prowl accepts both): A, the measurement includes Codex session logs and reports per harness; B, replay receipts never copy raw secret-shaped command text. Both are folded into the measurement section. |
| Operator | APPROVE | 2026-11-12 2:51 PM ET | Approved with soundwave's changes A and B (recorded on cas-24f2 by the prowl supervisor). |
| Supervisor (prowl) | PENDING (revision 2) | | |
| Supervisor (soundwave) | PENDING (revision 2) | | |
| Operator | PENDING (revision 2) | | |
