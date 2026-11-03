# Rule references in code and tests

When a task or its newest supervisor brief cites an existing stakeholder rule (one with stakeholder attribution), mark each code or test site written because of that rule. Citing a process rule without stakeholder attribution, such as a release-flow rule, explains the work and needs no marker. Keep unrelated changes outside the rule's scope. A marker is a standalone comment immediately before the affected source line:

```rust
// cas-rule: <rule-id> scope: "The stakeholder's verbatim scope"
fn send_sms() {}
```

The scope is a JSON string: quote and escape it with JSON, without paraphrasing. This business scope is distinct from the rule's project/global storage namespace. Supported single-line comment forms are `//`, `#`, `--`, `/* ... */` and `<!-- ... -->`. Several rule markers can form one adjacent block; blank lines are allowed, unrelated comments or code end that block. For Rust, place the marker after attributes and before the function or statement it governs.

Declare causal sites in the newest supervisor brief or the worker's close reason. A declaration is the field name followed by a JSON array on one line; a list bullet or other text before the field, and text after the array, are allowed. A malformed array refuses close and names its line. The title, description, design and acceptance criteria can quote the syntax without declaring sites.

```text
Rule-driven sites: [{"rule_id":"<rule-id>","path":"src/sms.rs","anchor":"fn send_sms() {}"},{"rule_id":"<rule-id>","path":"tests/sms.rs","anchor":"fn rejects_out_of_scope_send() {}"}]
```

Paths are repository-relative. An anchor is the exact trimmed source line and must be unique in that file. Supervisors can name sites before work starts; workers can provide the final anchors in their close receipt. A post-merge close preserves the parked worker receipt unless its new close reason supplies a new declaration. Transactional completion receipts can supply the same line in `scope_summary`.

Close checks the immutable delivered Git tip before parking and before accepting a transactional receipt. Each cited stakeholder rule and each rule named by a declared site needs at least one valid marker, and every declared site needs its adjacent marker. Missing, malformed or empty scope strings refuse close. When the rule serializes `stakeholder.scope`, every marker for that rule must match it exactly. A declared site for a rule without stakeholder attribution uses the marker's explicit scope; no scope is invented. Routine progress notes and superseded spawn briefs do not add citations.

The gate cannot infer why arbitrary code was written. The verifier reads the diff and rejects rule-driven code/tests omitted from the declared sites; one marker in an unrelated function is not complete evidence. A citation's presence does not prove that the code correctly implements its rule.

## Find a rule's sites

```bash
cas rules refs <rule-id>
cas rules refs <rule-id> --json
```

The read-only command needs a Git checkout, not a Cassy login or initialized database. It works from a subdirectory and scans tracked working-tree text, including unstaged edits. It excludes untracked files, binary files, symlinks and submodules. Paths are repository-relative; line numbers start at one. JSON is one sorted array of `{rule_id, path, line, scope}` objects, with complete values. Human rows fit 80 columns; `--full` or `--json` preserves long values. A malformed marker for the requested id fails with its path and line. Other rule ids do not affect the query.

Scanning recognizes comment syntax, not a language AST. Do not put real rule ids into marker-shaped example text: examples use `<rule-id>` so documentation cannot masquerade as a code reference. Review still establishes the marker's causal meaning.

## Stakeholder provenance integration

`rule_refs::stakeholder_scope` is the one enrichment seam. It directly reads cas-adeb's optional `Rule.stakeholder.scope`, using its existing `StakeholderAttribution {name, quote, scope}` type and migration m294 (governance base `46861faf`). It adds no provenance fields or migration. The real close regression stores that attribution and rejects a broader marker scope. Omitted update fields preserve attribution; an explicit MCP clear makes it absent, so legacy marker semantics apply. Follow-up: reference navigation can expose the existing stakeholder identity and quote.

## Cleanup when a rule changes

These rule changes list the rule's cited sites for review:

- `rule action=delete` retires the rule.
- `rule action=update` adds a `superseded` or `superseded-by:<rule-id>` tag.
- `rule action=update stakeholder=""` clears its attribution.
- `rule action=update stakeholder_scope=...` changes its scope; only markers recording a different scope are listed.
- Daemon maintenance finds that a stakeholder rule's `review_after` has passed. Unattributed rules are skipped because their review date is only a default hint.

Cassy scans the project's tracked working tree with the same scanner as `cas rules refs`. It files one chore for the checkout, titled `Review <n> <rule-id> citation(s) in <repo>`, listing each `path:line` and recorded scope. Its `external_ref` is `rule-cleanup:<rule-id>:<change>`, where `<change>` is the rule history version or the expired review date. A retry or a repeated sweep therefore returns the same chore. Nothing is filed when no site needs review or the project is not a Git checkout. The rule tool's reply names the chore, or the listing error; the stored rule change stands either way.

Cassy never edits or deletes a cited site. The chore's owner removes each marker together with the code written for it, or re-scopes it. Only the project checkout holding the rule store is scanned. For a global rule cited in other repositories, run `cas rules refs <rule-id>` in each one.
