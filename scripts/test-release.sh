#!/usr/bin/env bash
# Deterministic self-test for scripts/release.sh (and its CHANGELOG helper
# scripts/release_changelog.py), the one-command release
# (cas-f95b). A bare repository stands in for origin and a fake gh records
# every PR call, so nothing here reaches GitHub.
#
# Proves: the version is computed from the base branch; the bump, Cargo.lock,
# CHANGELOG promotion and draft land in one commit on the epic branch; the PR
# opens with auto-merge; when another release lands first the command merges
# the base, resolves the release files itself and re-bumps; a conflict outside
# the release files stops with exit 4; an ejected PR stops the watch with
# exit 3; a merged PR ends it with exit 0.
set -euo pipefail
# A git hook (the pre-push fast rows) exports GIT_DIR and friends. A fixture
# `git init` would then reinitialise the real repository (cas-f95b), so drop
# every repository-local git variable before touching a fixture repository.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars)

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }

mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
state="${FAKE_GH_STATE:?}"
case "$1 $2" in
  "repo view") echo example/repo ;;
  "pr list") [[ -f "$state/pr" ]] && cat "$state/pr" || true ;;
  "pr create") echo 77 >"$state/pr"; echo https://example.test/pull/77 ;;
  "pr edit") ;;
  "pr merge") exit "${FAKE_MERGE_EXIT:-0}" ;;
  "pr checks") cat "${FAKE_PR_CHECKS_JSON:?}" ;;
  "run view") cat "${FAKE_RUN_LOG:-/dev/null}" ;;
  "api graphql") cat "${FAKE_PR_STATE_JSON:?}" ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"

export GH_BIN="$tmp/bin/gh"
export FAKE_GH_LOG="$tmp/gh.log"
export FAKE_GH_STATE="$tmp/state"
export RELEASE_DATE=2026-10-15
export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
mkdir -p "$FAKE_GH_STATE"
export FAKE_PR_CHECKS_JSON="$tmp/checks-green.json"
echo '[{"name":"Scoped Validation (fast)","bucket":"pass","link":"https://example.test/actions/runs/1/job/11"},{"name":"Scoped Validation (factory/PR)","bucket":"skipping","link":"https://example.test/actions/runs/1/job/12"}]' >"$FAKE_PR_CHECKS_JSON"

crates=(cas-types cas-search cas-store cas-core cas-mcp)

write_tree() {
    local root="$1" version="$2" crate
    mkdir -p "$root/cas-cli" "$root/scripts" "$root/docs/release-notes"
    printf '[package]\nname = "cas"\nversion = "%s"\n\n[dependencies]\nserde = "1"\n' "$version" \
        >"$root/cas-cli/Cargo.toml"
    {
        printf 'version = 4\n\n'
        printf '[[package]]\nname = "cas"\nversion = "%s"\n\n' "$version"
        for crate in "${crates[@]}"; do
            mkdir -p "$root/crates/$crate"
            printf '[package]\nname = "%s"\nversion = "%s"\n' "$crate" "$version" >"$root/crates/$crate/Cargo.toml"
            printf '[[package]]\nname = "%s"\nversion = "%s"\n\n' "$crate" "$version"
        done
        printf '[[package]]\nname = "cas-diffs"\nversion = "0.1.0"\n'
    } >"$root/Cargo.lock"
    cat >"$root/CHANGELOG.md" <<EOF2
# Changelog

## [Unreleased]

## [$version] - 2026-10-14

- The base release.
EOF2
    cp "$script_dir/release.sh" "$script_dir/release_changelog.py" "$script_dir/release-train-announce.py" "$root/scripts/"
    # Real, dependency-free npm boundary: the fixture build embeds the crate
    # version just as Commander's Vite config does. No npm/Git calls are mocked.
    mkdir -p "$root/hub-web/dist"
    printf 'base styles\n' >"$root/hub-web/dist/app.css"
    printf '{"name":"release-web-fixture","version":"1.0.0","private":true,"scripts":{"build":"node build.cjs"}}\n' >"$root/hub-web/package.json"
    printf '{"name":"release-web-fixture","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"release-web-fixture","version":"1.0.0"}}}\n' >"$root/hub-web/package-lock.json"
    cat >"$root/hub-web/build.cjs" <<'JS'
const fs = require('node:fs');
const version = /^version = "([^"]+)"/m.exec(fs.readFileSync('../cas-cli/Cargo.toml', 'utf8'))[1];
const sources = fs.readdirSync('..').filter(name => name.endsWith('.txt')).sort().map(name => ({ name, content: fs.readFileSync(`../${name}`, 'utf8') }));
fs.mkdirSync('dist', { recursive: true });
fs.writeFileSync('dist/app.js', `const version = "${version}";\nconst sources = ${JSON.stringify(sources)};\n`);
JS
    echo base >"$root/app.txt"
    (cd "$root/hub-web" && node build.cjs)
}

write_valid_draft() {
    local path="$1" name="${1##*/}"
    cat >"$path" <<'EOF2'
```text
*Live on production — User — Cassy v{{VERSION}}*
Was: DRAFT_NAME release notes were harder to use. → Now: this release is easier to use.
```

```text
• *Clearer flow* — The release is easier to follow.
```

```text
*Live on production — Dev — Cassy v{{VERSION}}*
Was: release prep lacked one check. → Now: release prep runs it before the PR.
```

```text
• *Release check* — The release cut validates the draft before publishing.
```
EOF2
    sed -i "s/DRAFT_NAME/$name/" "$path"
}

git init --quiet --bare -b main "$tmp/origin.git"
git init --quiet -b main "$tmp/seed"
write_tree "$tmp/seed" 9.99.0
git -C "$tmp/seed" add -A
git -C "$tmp/seed" commit --quiet -m base
git -C "$tmp/seed" remote add origin "$tmp/origin.git"
git -C "$tmp/seed" push --quiet origin main
git -C "$tmp/seed" tag -a v9.99.0 -m v9.99.0
git -C "$tmp/seed" push --quiet origin v9.99.0

# An epic branch with its own entries and draft.
clone_epic() {
    local dir="$1" name="$2" entry="$3"
    git clone --quiet "$tmp/origin.git" "$dir"
    git -C "$dir" checkout --quiet -b "$name"
    perl -0777 -i -pe "s/## \\[Unreleased\\]\\n/## [Unreleased]\\n\\n### Added — $entry\\n\\n- $entry shipped.\\n/" "$dir/CHANGELOG.md"
    echo "$entry" >"$dir/$entry.txt"
    printf '%s styles\n' "$entry" >"$dir/hub-web/dist/app.css"
    git -C "$dir" add -A
    git -C "$dir" commit --quiet -m "$entry work"
}

clone_epic "$tmp/epic" epic/alpha alpha
epic="$tmp/epic"
run() { (cd "$1" && shift && ./scripts/release.sh "$@"); }

# Delivery anchors live in the selected clone's store, including worktrees.
record_delivery() {
    python3 - "$1" "$2" "$3" "$4" <<'PYDB'
import json, sqlite3, sys
from pathlib import Path
root, task, anchor, base = sys.argv[1:]
folder = Path(root) / '.cas'
folder.mkdir(exist_ok=True)
with sqlite3.connect(folder / 'cas.db') as db:
    db.execute('CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, deliverables TEXT)')
    db.execute('INSERT OR REPLACE INTO tasks VALUES (?, ?)',
               (task, json.dumps({'factory_branch_anchor': anchor,
                                  'factory_branch_start': {'base_sha': base}})))
with (Path(root) / '.git/info/exclude').open('a') as out:
    out.write('\n.cas/\n')
PYDB
}

missing_fragment_delivery_stops_release() {
    local fixture="$tmp/missing-fix" base fix before status=0
    clone_epic "$fixture" hotfix/missing-fix missing-fix
    base="$(git -C "$fixture" rev-parse HEAD)"
    echo 'actual peer wake fix' >"$fixture/fix.txt"
    git -C "$fixture" add fix.txt
    git -C "$fixture" commit --quiet -m 'peer wake fix'
    fix="$(git -C "$fixture" rev-parse HEAD)"
    git -C "$fixture" reset --quiet --hard "$base"
    mkdir -p "$fixture/changelog.d"
    printf '### Fixed — Peer wake\n\n- **Wake** — the peer wake fix shipped.\n' >"$fixture/changelog.d/cas-0004.md"
    git -C "$fixture" add changelog.d
    git -C "$fixture" commit --quiet -m 'notes still claim the dropped fix'
    record_delivery "$fixture" cas-0004 "$fix" "$base"
    write_valid_draft "$tmp/missing-fix-draft.md"
    before="$(git -C "$fixture" rev-parse HEAD)"
    mkdir -p "$tmp/missing-fix-state"
    : >"$tmp/missing-fix-gh.log"
    FAKE_GH_STATE="$tmp/missing-fix-state" FAKE_GH_LOG="$tmp/missing-fix-gh.log" \
        run "$fixture" --hotfix --no-watch --notes "$tmp/missing-fix-draft.md" >"$tmp/missing-fix.log" 2>&1 || status=$?
    [[ $status -ne 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] &&
        grep -q 'cas-0004' "$tmp/missing-fix.log" && grep -q "$fix" "$tmp/missing-fix.log" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/hotfix/missing-fix &&
        ! grep -q '^pr create' "$tmp/missing-fix-gh.log" || return 1

    # A cherry-pick changes identity but retains the full delivery patch.
    git -C "$fixture" cherry-pick --quiet "$fix" || return 1
    run "$fixture" --hotfix --dry-run --notes "$tmp/missing-fix-draft.md" >>"$tmp/missing-fix.log" 2>&1 || return 1
    [[ ! -f "$fixture/changelog.d/cas-0004.md" ]] || return 1
    # A resumed release must recover the consumed fragment's claim, even
    # after the code is dropped from an already-prepared branch.
    git -C "$fixture" rm --quiet fix.txt
    git -C "$fixture" commit --quiet -m 'drop fix after preparation'
    before="$(git -C "$fixture" rev-parse HEAD)"
    status=0
    FAKE_GH_STATE="$tmp/missing-fix-state" FAKE_GH_LOG="$tmp/missing-fix-gh.log" \
        run "$fixture" --hotfix --no-watch >>"$tmp/missing-fix.log" 2>&1 || status=$?
    [[ $status -ne 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] &&
        grep -q "$fix" "$tmp/missing-fix.log" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/hotfix/missing-fix &&
        ! grep -q '^pr create' "$tmp/missing-fix-gh.log"
}
check 'missing_fragment_delivery_stops_release' 'cas-4bdf: missing fragment fix refuses before commit, push or PR and names task and anchor'

# cas-56bbc: a dry patch release must commit the bundle built from the new
# manifest, not leave the old embedded version for the merge queue to reject.
dry_release_rebuilds_commander() {
    local fixture="$tmp/web-release" committed
    clone_epic "$fixture" epic/web-release web-release
    write_valid_draft "$tmp/web-draft.md"
    run "$fixture" --hotfix --dry-run --notes "$tmp/web-draft.md" >"$tmp/web-release.log" 2>&1 || return 1
    committed="$(git -C "$fixture" show HEAD:hub-web/dist/app.js)"
    (cd "$fixture/hub-web" && npm ci && npm run build) >>"$tmp/web-release.log" 2>&1 || return 1
    [[ "$committed" == *'"9.99.1"'* ]] &&
        git -C "$fixture" diff --quiet HEAD -- hub-web/dist &&
        [[ -z "$(git -C "$fixture" status --porcelain)" ]] &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/web-release
}
check 'dry_release_rebuilds_commander' 'cas-56bbc: dry release commits Commander dist identical to a fresh post-bump build'

commander_build_failure_stops_release() {
    local fixture="$tmp/web-failure" before status=0
    clone_epic "$fixture" epic/web-failure web-failure
    printf 'process.exit(17);\n' >"$fixture/hub-web/build.cjs"
    git -C "$fixture" commit --quiet -am 'broken web source'
    before="$(git -C "$fixture" rev-parse HEAD)"
    run "$fixture" --hotfix --dry-run --notes "$tmp/web-draft.md" >"$tmp/web-failure.log" 2>&1 || status=$?
    [[ $status -ne 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] &&
        grep -q 'Commander build failed; release was not committed' "$tmp/web-failure.log" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/web-failure
}
check 'commander_build_failure_stops_release' 'a failed Commander build stops before the release commit or push'

invalid_draft_stops_cut() {
    local fixture="$tmp/invalid-draft" before status=0
    clone_epic "$fixture" epic/invalid-draft invalid-draft
    cat >"$tmp/invalid-slack.md" <<'EOF2'
```text
*Live on production — User — Cassy v{{VERSION}}*
Was: a supervisor must repair this broken draft before release. → Now: it still fails lint.
```

```text
• *Detail* — Still a valid reply.
```

```text
*Live on production — Dev — Cassy v{{VERSION}}*
Was: release prep lacked one check. → Now: release prep runs it before the PR.
```

```text
• *Release check* — The release cut validates the draft before publishing.
```
EOF2
    before="$(git -C "$fixture" rev-parse HEAD)"
    : >"$FAKE_GH_LOG"
    run "$fixture" --no-watch --notes "$tmp/invalid-slack.md" >"$tmp/invalid-draft.log" 2>&1 || status=$?
    [[ $status -ne 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] &&
        grep -q 'forbidden user wording: supervisor' "$tmp/invalid-draft.log" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/invalid-draft &&
        ! grep -q '^pr create' "$FAKE_GH_LOG"
}
check 'invalid_draft_stops_cut' 'a release draft failing lint is refused before commit, push, or PR creation'

# 1. The version comes from the base branch.
out="$(run "$epic" --next-version)"
check '[[ "$out" == *"version=9.100.0"* && "$out" == *"draft=docs/release-notes/2026-10-15-v9.100.0-slack.md"* ]]' '--next-version prints the next minor version and its draft path'
out="$(run "$epic" --next-version --hotfix)"
check '[[ "$out" == *"version=9.99.1"* ]]' '--hotfix takes the next patch version'
check '[[ -z "$(git -C "$epic" status --porcelain)" ]]' '--next-version changes nothing'

# 2. No draft: a named refusal before any change.
set +e
out="$(run "$epic" --no-watch 2>&1)"
status=$?
set -e
check '[[ $status -eq 1 && "$out" == *"no release-notes draft: write docs/release-notes/2026-10-15-v9.100.0-slack.md"* ]]' 'a missing draft is refused with the path to write'
check '[[ "$(git -C "$epic" log -1 --format=%s)" == "alpha work" && -z "$(git -C "$epic" status --porcelain)" ]]' 'a refused release changes and commits nothing'

# 3. The release commit, the PR and auto-merge.
original_notes=docs/release-notes/2026-10-14-v9.100.0-slack.md
mkdir -p "$epic/docs/release-notes"
write_valid_draft "$epic/$original_notes"
# cas-f2be: a per-delivery fragment rides into the version section.
mkdir -p "$epic/changelog.d"
printf 'How fragments work.\n' >"$epic/changelog.d/README.md"
printf '### Fixed — Fragments\n\n- **Fragment entry** — fragment shipped,\n  on a wrapped line.\n' >"$epic/changelog.d/cas-0001.md"
# cas-7f95: same-area fragments (and the epic's own [Unreleased] area) share one heading.
printf '### Added — alpha\n\n- **Second alpha** — the follow-up landed.\n' >"$epic/changelog.d/cas-0002.md"
printf '### Fixed — Fragments\n\n- **Third fragment** — third fragment shipped.\n' >"$epic/changelog.d/cas-0003.md"
git -C "$epic" add "$original_notes" changelog.d
git -C "$epic" commit --quiet -m 'release draft'
for task in cas-0001 cas-0002 cas-0003; do
    record_delivery "$epic" "$task" "$(git -C "$epic" rev-parse HEAD^)" "$(git -C "$epic" rev-parse HEAD^^)"
done
: >"$FAKE_GH_LOG"
run "$epic" --no-watch --notes "$original_notes" 2>"$tmp/run.err" || { cat "$tmp/run.err"; exit 1; }
head_files="$(git -C "$epic" show --name-only --format= HEAD | sort | paste -sd' ' -)"
check '[[ "$(git -C "$epic" log -1 --format=%s)" == "release: v9.100.0" ]]' 'the release commit is on the epic branch'
check 'grep -q "^version = \"9.100.0\"" "$epic/cas-cli/Cargo.toml" && grep -q "^version = \"9.100.0\"" "$epic/crates/cas-mcp/Cargo.toml"' 'every release crate is bumped'
check '[[ "$(grep -c "^version = \"9.100.0\"" "$epic/Cargo.lock")" -eq 6 ]] && grep -q "^version = \"0.1.0\"" "$epic/Cargo.lock"' 'Cargo.lock bumps the release packages and nothing else'
check 'grep -A16 "^## \[9.100.0\] - 2026-10-15" "$epic/CHANGELOG.md" | grep -q "alpha shipped"' 'CHANGELOG promotes [Unreleased] to the dated version'
check 'awk "/^## \\[Unreleased\\]/{getline; getline; print; exit}" "$epic/CHANGELOG.md" | grep -q "^## \[9.100.0\]"' '[Unreleased] is left empty'
check 'sed -n "/^## \[9.100.0\]/,/^## \[9.99.0\]/p" "$epic/CHANGELOG.md" | grep -q "fragment shipped" && [[ "$(git -C "$epic" ls-files changelog.d)" == changelog.d/README.md ]] && [[ -z "$(git -C "$epic" status --porcelain)" ]]' 'cas-f2be: changelog.d fragments fold into the version section and the release commit deletes them'
released_section="$(sed -n '/^## \[9.100.0\]/,/^## \[9.99.0\]/p' "$epic/CHANGELOG.md")"
heading_line() { grep -n -m1 -F -- "$1" <<<"$released_section" | cut -d: -f1; }
check '[[ "$(grep -c "^### Fixed — Fragments$" <<<"$released_section")" -eq 1 && "$(grep -c "^### Added — alpha$" <<<"$released_section")" -eq 1 ]]' 'cas-7f95: entries sharing a section title share one heading'
check '(( $(heading_line "### Fixed — Fragments") < $(heading_line "third fragment shipped") && $(heading_line "third fragment shipped") < $(heading_line "### Added — alpha") && $(heading_line "### Added — alpha") < $(heading_line "- alpha shipped.") ))' 'cas-7f95: headings keep first-seen order and gather every entry under them'
check '[[ "$(heading_line "third fragment shipped")" -eq $(( $(heading_line "on a wrapped line.") + 1 )) ]]' 'cas-7f95: a bullet after a wrapped bullet stays in the same list'
check 'python3 "$script_dir/markdownlint.py" --config "$script_dir/../.markdownlint-cli2.jsonc" "$epic/CHANGELOG.md" >"$tmp/lint.out" 2>&1 || { cat "$tmp/lint.out"; false; }' 'cas-7f95: the promoted CHANGELOG passes the Docs Lint markdownlint policy (MD024)'
check 'grep -q "Live on production — User — Cassy v9.100.0" "$epic/docs/release-notes/2026-10-15-v9.100.0-slack.md"' 'the draft is named and filled for the version'
check '[[ "$head_files" == *CHANGELOG.md* && "$head_files" == *Cargo.lock* && "$head_files" == *slack.md* ]]' 'one commit carries bump, lock, CHANGELOG and draft'
check '[[ "$(git -C "$tmp/origin.git" rev-parse refs/heads/epic/alpha)" == "$(git -C "$epic" rev-parse HEAD)" ]]' 'the release commit is pushed'
check 'grep -q "^pr create --base main --head epic/alpha --title Release v9.100.0: epic/alpha" "$FAKE_GH_LOG"' 'the PR to main is opened'
check 'grep -q "^pr merge 77 --auto --merge" "$FAKE_GH_LOG"' 'auto-merge is enabled'
checked="$(grep -n "^pr checks 77" "$FAKE_GH_LOG" | head -1 | cut -d: -f1 || true)"
merged="$(grep -n "^pr merge 77" "$FAKE_GH_LOG" | head -1 | cut -d: -f1)"
check '[[ -n "$checked" && -n "$merged" && "$checked" -lt "$merged" ]]' 'the PR CI is green before auto-merge is enabled (cas-2aef)'
check '! git -C "$tmp/origin.git" show-ref --tags | grep -q v9.100.0' 'nothing is tagged'

# 3b. cas-2aef: a red PR CI is reported with its failing tests, and the PR never
# gets auto-merge, so it never enters the queue.
echo '[{"name":"Scoped Validation (fast)","bucket":"fail","link":"https://example.test/actions/runs/2/job/21"},{"name":"Scoped Validation (factory/PR)","bucket":"pass","link":"https://example.test/actions/runs/2/job/22"}]' >"$tmp/checks-red.json"
printf 'Scoped Validation (fast)\tRun shards\t2026-10-15T14:02:03.1234567Z         FAIL [   0.012s] cas-mux opencode::tests::projection_config_is_deterministic\nScoped Validation (fast)\tRun shards\t2026-10-15T14:02:03.2Z test release::tests::bumps ... FAILED\nScoped Validation (fast)\tRun shards\t2026-10-15T14:02:04.0Z      FAIL [   0.012s] cas-mux opencode::tests::projection_config_is_deterministic\n' >"$tmp/red.log"
: >"$FAKE_GH_LOG"
set +e
out="$(cd "$epic" && FAKE_PR_CHECKS_JSON="$tmp/checks-red.json" FAKE_RUN_LOG="$tmp/red.log" ./scripts/release.sh --no-watch 2>&1)"
status=$?
set -e
check '[[ $status -eq 5 && "$out" == *"CI is red (Scoped Validation (fast))"* ]]' 'a red PR CI stops with exit 5 and names the failing check'
check '[[ "$out" == *"Failing tests: cas-mux opencode::tests::projection_config_is_deterministic; release::tests::bumps"* ]]' 'the failing tests are named once, in one message'
check '! grep -q "^pr merge" "$FAKE_GH_LOG" && grep -q "^run view --job 21 --log-failed" "$FAKE_GH_LOG"' 'a red PR gets no auto-merge'

# 4. cas-3873: after an ejection repair, rerun the same command even on a
# later day. Both the original (now removed) and renamed notes paths work.
rerun_after_repair() {
    local before notes out status
    echo repaired >"$epic/alpha.txt"
    git -C "$epic" commit --quiet -am 'repair queue failure'
    before="$(git -C "$epic" rev-parse HEAD)"
    for notes in "$original_notes" docs/release-notes/2026-10-15-v9.100.0-slack.md ''; do
        : >"$FAKE_GH_LOG"
        out="$(cd "$epic" && RELEASE_DATE=2026-10-16 ./scripts/release.sh --no-watch ${notes:+--notes "$notes"} 2>&1)" || { echo "$out"; return 1; }
        [[ "$(git -C "$epic" rev-parse HEAD)" == "$before" && -z "$(git -C "$epic" status --porcelain)" ]] || return 1
        [[ "$(git -C "$tmp/origin.git" rev-parse refs/heads/epic/alpha)" == "$before" ]] || return 1
        grep -q '^pr edit 77 --title Release v9.100.0:' "$FAKE_GH_LOG" &&
            grep -q '^pr merge 77 --auto --merge' "$FAKE_GH_LOG" &&
            ! grep -q '^pr create' "$FAKE_GH_LOG" || return 1
    done
    status=0
    out="$(cd "$epic" && FAKE_MERGE_EXIT=42 ./scripts/release.sh --no-watch 2>&1)" || status=$?
    [[ $status -eq 42 && "$out" == *"error:"* && "$out" == *"42"* && "$out" == *'pr merge'* ]]
}
check 'rerun_after_repair' 'a repaired release requeues once with old/new notes paths and reports command failures'

# 5. Another epic's release lands first with the same version: re-bump.
clone_epic "$tmp/beta" epic/beta beta
# cas-9cb3: base deletes a fixed-name generated file our branch modified.
git -C "$tmp/beta" rm --quiet hub-web/dist/app.css
git -C "$tmp/beta" commit --quiet -m 'drop obsolete generated styles'
write_valid_draft "$tmp/beta-draft.md"
rm -f "$FAKE_GH_STATE/pr"
run "$tmp/beta" --no-watch --notes "$tmp/beta-draft.md" 2>/dev/null
git -C "$tmp/beta" push --quiet origin epic/beta:main
rm -f "$FAKE_GH_STATE/pr"; echo 77 >"$FAKE_GH_STATE/pr"
: >"$FAKE_GH_LOG"
run "$epic" --no-watch 2>"$tmp/rebump.err" || { cat "$tmp/rebump.err" >&2; exit 1; }
check '[[ "$(git -C "$epic" log -1 --format=%s)" == "release: v9.101.0" ]]' 'the re-bump takes the next free version'
check 'git -C "$epic" merge-base --is-ancestor origin/main HEAD' 'the base branch is merged in'
check 'grep -q "^## \[9.101.0\] - 2026-10-15" "$epic/CHANGELOG.md" && grep -A16 "^## \[9.101.0\]" "$epic/CHANGELOG.md" | grep -q "alpha shipped"' 'our entries move to the new version'
check 'grep -A4 "^## \[9.100.0\]" "$epic/CHANGELOG.md" | grep -q "beta shipped" && ! grep -A6 "^## \[9.100.0\]" "$epic/CHANGELOG.md" | grep -q "alpha shipped"' 'the landed release section is kept intact'
check '[[ "$(grep -c "alpha shipped" "$epic/CHANGELOG.md")" -eq 1 && "$(grep -c "^## \[9.100.0\]" "$epic/CHANGELOG.md")" -eq 1 ]]' 'no entry or heading is duplicated'
check '! grep -q "<<<<<<<\|>>>>>>>" "$epic/CHANGELOG.md" "$epic/Cargo.lock" "$epic/cas-cli/Cargo.toml"' 'no conflict markers remain'
check '[[ "$(grep -c "^version = \"9.101.0\"" "$epic/Cargo.lock")" -eq 6 ]] && grep -q "^version = \"9.101.0\"" "$epic/crates/cas-core/Cargo.toml"' 'manifests and lock carry the re-bumped version'
check 'grep -q "Cassy v9.101.0" "$epic/docs/release-notes/2026-10-15-v9.101.0-slack.md" && grep -q "Cassy v9.100.0" "$epic/docs/release-notes/2026-10-15-v9.100.0-slack.md"' 'our draft is renamed and the landed draft is untouched'
check '[[ -f "$epic/beta.txt" && -f "$epic/alpha.txt" ]]' 'the landed code is merged'
check 'grep -q "^pr edit 77 --title Release v9.101.0: epic/alpha" "$FAKE_GH_LOG" && grep -q "^pr merge 77 --auto --merge" "$FAKE_GH_LOG"' 'the open PR is retitled and auto-merge re-enabled'
check 'grep -q "9.101.0" "$epic/hub-web/dist/app.js" && grep -q "alpha" "$epic/hub-web/dist/app.js" && grep -q "beta" "$epic/hub-web/dist/app.js" && git -C "$epic" diff --quiet HEAD -- hub-web/dist' 'a re-bump resolves generated bundle conflicts and commits the merged web sources with the new version'
check '[[ ! -e "$epic/hub-web/dist/app.css" && -z "$(git -C "$epic" status --porcelain)" && ! -f "$epic/.git/MERGE_HEAD" ]]' 'cas-9cb3: a base-deleted fixed-name dist file resolves and the re-bump finishes cleanly'

# 6. A conflict outside the release files stops with exit 4 and no merge.
echo main-change >"$tmp/seed/app.txt"
git -C "$tmp/seed" pull --quiet --no-rebase origin main
echo main-change >"$tmp/seed/app.txt"
git -C "$tmp/seed" commit --quiet -am 'main edits app'
git -C "$tmp/seed" push --quiet origin main
echo epic-change >"$epic/app.txt"
git -C "$epic" commit --quiet -am 'epic edits app'
set +e
out="$(run "$epic" --no-watch 2>&1)"
status=$?
set -e
check '[[ $status -eq 4 && "$out" == *"conflicts outside the release files"* && "$out" == *app.txt* ]]' 'a code conflict stops with exit 4 and names the file'
check '[[ -z "$(git -C "$epic" status --porcelain)" && ! -f "$epic/.git/MERGE_HEAD" ]]' 'a code conflict leaves no merge in progress'
git -C "$epic" reset --quiet --hard HEAD^

# 7. Watch: an ejected PR stops with exit 3; a merged PR ends with exit 0.
git -C "$tmp/seed" revert --no-edit HEAD >/dev/null
git -C "$tmp/seed" push --quiet origin main
run "$epic" --no-watch 2>/dev/null
echo '{"state":"OPEN","mergeable":"MERGEABLE","autoMergeRequest":null,"mergeQueueEntry":null,"mergeCommit":null}' >"$tmp/ejected.json"
set +e
out="$(cd "$epic" && RELEASE_WATCH_INTERVAL=0 FAKE_PR_STATE_JSON="$tmp/ejected.json" ./scripts/release.sh 2>&1)"
status=$?
set -e
check '[[ $status -eq 3 && "$out" == *"no auto-merge and no merge-queue entry"* ]]' 'an ejected PR stops the watch with exit 3'
echo '{"state":"OPEN","mergeable":"MERGEABLE","autoMergeRequest":null,"mergeQueueEntry":{"state":"QUEUED"},"mergeCommit":null}' >"$tmp/queued.json"
echo '{"state":"MERGED","mergeable":"UNKNOWN","autoMergeRequest":null,"mergeQueueEntry":null,"mergeCommit":{"oid":"abc123"}}' >"$tmp/merged.json"
cat >"$tmp/bin/gh-seq" <<EOF
#!/usr/bin/env bash
if [[ "\$1 \$2" == "api graphql" ]]; then
  n="\$(cat "$tmp/seq" 2>/dev/null || echo 0)"; n=\$((n + 1)); echo "\$n" >"$tmp/seq"
  if [[ "\$n" -lt 3 ]]; then cat "$tmp/queued.json"; else cat "$tmp/merged.json"; fi
  exit 0
fi
exec "$tmp/bin/gh" "\$@"
EOF
chmod +x "$tmp/bin/gh-seq"
set +e
out="$(cd "$epic" && GH_BIN="$tmp/bin/gh-seq" RELEASE_WATCH_INTERVAL=0 ./scripts/release.sh 2>&1)"
status=$?
set -e
check '[[ $status -eq 0 && "$out" == *"merged as abc123"* ]]' 'a queued then merged PR ends the watch with exit 0'

# 8. cas-ea62: git exports GIT_DIR to hooks. A fixture
# test that creates bare repositories must leave the repository those name
# untouched (it once set core.bare=true on the shared checkout).
parent="$tmp/parent"
git init --quiet "$parent"
git -C "$parent" commit --quiet --allow-empty -m parent
cp "$parent/.git/config" "$tmp/parent-config.before"
(
    # The pre-push hook exports GIT_DIR alone, as git does for hooks.
    export GIT_DIR="$parent/.git"
    "$script_dir/test-release-publish.sh" >/dev/null 2>&1
) || bad 'the bare-fixture test passes under exported hook variables'
check 'cmp -s "$tmp/parent-config.before" "$parent/.git/config"' 'a bare fixture under an exported GIT_DIR leaves the parent config untouched'
check '[[ "$(git -C "$parent" rev-list --count HEAD)" -eq 1 && -z "$(git -C "$parent" tag)" ]]' 'a bare fixture under an exported GIT_DIR adds no commit or tag to the parent'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
