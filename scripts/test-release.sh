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
  "release view") exit 1 ;;
  "release create") ;;
  "api repos/"*)
    case "$2" in */logs) cat "${FAKE_TRAIN_LOG_ZIP:?}" ;; *) echo '{}' ;; esac ;;
  "pr list") [[ -f "$state/pr" ]] && cat "$state/pr" || true ;;
  "pr create") echo 77 >"$state/pr"; echo https://example.test/pull/77 ;;
  "pr edit") ;;
  "pr merge") [[ "${FAKE_MERGE_EXIT:-0}" == 0 ]] && touch "$state/admitted"; exit "${FAKE_MERGE_EXIT:-0}" ;;
  "pr checks") cat "${FAKE_PR_CHECKS_JSON:?}" ;;
  "run view") cat "${FAKE_RUN_LOG:-/dev/null}" ;;
  "run download") cp -R "${FAKE_TRAIN_ARTIFACT:?}/." "${@: -1}" ;;
  "api -X")
    case "$4" in
      */jobs) echo '{"jobs":[{"name":"Shared train and queue graph / Full-set gate","conclusion":"success"}]}' ;;
      *) cat "${FAKE_TRAIN_RUN:-/dev/null}" ;;
    esac ;;
  "api graphql")
    if [[ -n "${FAKE_PR_STATE_JSON:-}" ]]; then cat "$FAKE_PR_STATE_JSON"
    elif [[ -f "$state/admitted" ]]; then echo '{"state":"OPEN","autoMergeRequest":null,"mergeQueueEntry":{"state":"QUEUED"}}'
    else echo '{"state":"OPEN","autoMergeRequest":null,"mergeQueueEntry":null}'; fi | jq --argjson config "${FAKE_QUEUE_CONFIG:-$(printf '%s' '{"maximumEntriesToMerge":1,"mergingStrategy":"ALLGREEN"}')}" '. + {queueConfiguration: $config}' ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"

cat >"$tmp/bin/cas" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_CAS_LOG:?}"
case "$1 $2" in
  "release context")
    case "${FAKE_CONTEXT_MODE:-}" in
      no_train) echo 'Error: no open release train; create work on the train first' >&2; exit 1 ;;
      different_branch) echo 'Error: check out the open release train epic/train-cas-a001 first' >&2; exit 1 ;;
    esac
    printf '{"train_id":"cas-0001","branch":"%s","integration":"main"}\n' "$(git branch --show-current)" ;;
  "release preview")
    jq -n --arg sha "$(git rev-parse HEAD)" --argjson n "${FAKE_WOULD_CLOSE:-1}" '{scope:{final_release_sha:$sha},members:2,would_close:$n,would_hold:(2-$n),by_reason:{REVIEW_MISSING:(2-$n)},review_missing:["cas-0001"]}' ;;
  "qa candidate")
    case "$3" in build) ;; path) printf '%s\n' "$0" ;; *) exit 2 ;; esac ;;
  "release track") exit "${FAKE_TRACK_EXIT:-0}" ;;
  "ledger estimate") echo 'typical 16 min, bad day 31 min; 20 references'; exit "${FAKE_ESTIMATE_EXIT:-0}" ;;
  *) echo "unexpected fake cas invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/cas"
export CAS_BIN="$tmp/bin/cas" FAKE_CAS_LOG="$tmp/cas.log"

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
    cp "$script_dir/release.sh" "$script_dir/release_changelog.py" "$script_dir/release-train-announce.py" "$script_dir/github_queue_health.py" "$script_dir/github-queue-health.graphql" "$script_dir/train-ci.py" "$script_dir/classify-ci-diff.sh" "$script_dir/find-release-prebuild.sh" "$root/scripts/"
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
    name="${name%.md}" # a file name in the body would trip the hub's unattached-file rule
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
    sed "s/DRAFT_NAME/$name/" "$path" > "$path.tmp" && mv "$path.tmp" "$path"
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
# The train starts above a newer main commit than a stale local origin/main.
git -C "$tmp/seed" commit --quiet --allow-empty -m 'unrelated main work'
git -C "$tmp/seed" push --quiet origin main

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
epic_first_child="$(git -C "$epic" rev-parse HEAD)"
run() { (cd "$1" && shift && ./scripts/release.sh "$@"); }

missing_fragment_delivery_stops_release() {
    local fixture="$tmp/missing-fix" base fix second before status=0
    clone_epic "$fixture" hotfix/missing-fix missing-fix
    # Replay af4c35f71 (red) plus d702e7333/8890a7a95 (notes), without
    # their fix: the fragment rides in the fix commit and is dropped too.
    echo 'failing peer wake regression' >"$fixture/red-test.txt"
    git -C "$fixture" add red-test.txt
    git -C "$fixture" commit --quiet -m 'test(peer): untyped peer mail must wake a peer (cas-0004 red)'
    git -C "$fixture" commit --quiet --allow-empty -m 'test: another missing delivery (cas-0005, red)'
    base="$(git -C "$fixture" rev-parse HEAD)"
    mkdir -p "$fixture/changelog.d"
    echo 'actual peer wake fix' >"$fixture/fix.txt"
    printf '### Fixed — Peer wake\n\n- **Wake** — the peer wake fix shipped.\n' >"$fixture/changelog.d/cas-0004.md"
    printf '# delivery-lock-marker\n' >>"$fixture/Cargo.lock"
    echo 'delivery generated bundle' >"$fixture/hub-web/dist/app.js"
    git -C "$fixture" add fix.txt changelog.d Cargo.lock hub-web/dist
    git -C "$fixture" commit --quiet -m 'fix: peer wake and its fragment (cas-0004)'
    fix="$(git -C "$fixture" rev-parse HEAD)"
    git -C "$fixture" branch delivered-peer "$fix"
    echo second >"$fixture/second.txt"
    git -C "$fixture" add second.txt
    git -C "$fixture" commit --quiet -m 'fix: second delivery (cas-0005)'
    second="$(git -C "$fixture" rev-parse HEAD)"
    git -C "$fixture" branch delivered-second "$second"
    git -C "$fixture" reset --quiet --hard "$base"
    mkdir -p "$fixture/docs/release-notes"
    write_valid_draft "$fixture/docs/release-notes/2026-10-14-v9.99.1-slack.md"
    git -C "$fixture" add docs/release-notes
    git -C "$fixture" commit --quiet -m 'docs(release): Slack draft'
    sed 's/easier to use/safer to use/' "$fixture/docs/release-notes/2026-10-14-v9.99.1-slack.md" > "$fixture/draft.tmp" &&
        mv "$fixture/draft.tmp" "$fixture/docs/release-notes/2026-10-14-v9.99.1-slack.md"
    git -C "$fixture" commit --quiet -am 'docs(release): ships the peer wake fix only'
    [[ ! -f "$fixture/changelog.d/cas-0004.md" && ! -f "$fixture/fix.txt" ]] || return 1
    before="$(git -C "$fixture" rev-parse HEAD)"
    mkdir -p "$tmp/missing-fix-state"
    : >"$tmp/missing-fix-gh.log"
    FAKE_GH_STATE="$tmp/missing-fix-state" FAKE_GH_LOG="$tmp/missing-fix-gh.log" \
        run "$fixture" --hotfix --no-watch --notes "$fixture/docs/release-notes/2026-10-14-v9.99.1-slack.md" >"$tmp/missing-fix.log" 2>&1 || status=$?
    [[ $status -ne 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] &&
        grep -q 'cas-0004' "$tmp/missing-fix.log" && grep -q "$fix" "$tmp/missing-fix.log" &&
        grep -q 'cas-0005' "$tmp/missing-fix.log" && grep -q "$second" "$tmp/missing-fix.log" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/hotfix/missing-fix &&
        ! grep -q '^pr create' "$tmp/missing-fix-gh.log" || return 1

    # A cherry-pick changes identity but retains the full delivery patch.
    git -C "$fixture" cherry-pick --quiet "$fix" || return 1
    git -C "$fixture" commit --quiet --amend -m 'backport peer wake without the original subject'
    git -C "$fixture" cherry-pick --quiet "$second" || return 1
    ! git -C "$fixture" merge-base --is-ancestor "$fix" HEAD || return 1
    sed 's/actual peer wake fix/later refinement/' "$fixture/fix.txt" > "$fixture/fix.txt.tmp" &&
        mv "$fixture/fix.txt.tmp" "$fixture/fix.txt"
    git -C "$fixture" commit --quiet -am 'docs: mention cas-dead1 in passing'
    git -C "$fixture" checkout --quiet -b epic/cas-dead2
    echo merged >"$fixture/merge.txt"
    git -C "$fixture" add merge.txt
    git -C "$fixture" commit --quiet -m 'unrelated epic work'
    git -C "$fixture" checkout --quiet hotfix/missing-fix
    git -C "$fixture" merge --quiet --no-ff epic/cas-dead2 -m 'Merge epic/cas-dead2 (cas-dead3)'
    # Generated output and lock resolution may legitimately differ on a cut.
    sed 's/delivery-lock-marker/regenerated-lock-marker/' "$fixture/Cargo.lock" > "$fixture/Cargo.lock.tmp" &&
        mv "$fixture/Cargo.lock.tmp" "$fixture/Cargo.lock"
    echo 'regenerated bundle' >"$fixture/hub-web/dist/app.js"
    git -C "$fixture" commit --quiet -am 'regenerate build outputs'
    run "$fixture" --hotfix --dry-run --notes "$fixture/docs/release-notes/2026-10-14-v9.99.1-slack.md" >>"$tmp/missing-fix.log" 2>&1 || return 1
    [[ ! -f "$fixture/changelog.d/cas-0004.md" ]] || return 1
    # A later edit does not erase the historical delivery on a rerun.
    before="$(git -C "$fixture" rev-parse HEAD)"
    run "$fixture" --hotfix --dry-run >>"$tmp/missing-fix.log" 2>&1 &&
        [[ "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]]

}
check 'missing_fragment_delivery_stops_release || { cat "$tmp/missing-fix.log"; false; }' 'cas-4bdf: report all missing fixes; accept historical cherry-picks after edits and ignore epic mentions'

# Real release history is part of this regression: CI checks out full history.
historical_release_replay() {
    local status=0
    python3 - "$script_dir/.." "$tmp/history" >"$tmp/history.log" 2>&1 <<'PYHISTORY' || status=$?
from pathlib import Path
import shutil, subprocess, sys
root, clone = map(Path, sys.argv[1:])
def git(*args, cwd=root):
    return subprocess.check_output(['git', *args], cwd=cwd, text=True).strip()
subprocess.run(['git', 'clone', '--quiet', '--shared', '--no-checkout', str(root), str(clone)], check=True)
cases = []
for merge in git('log', '--first-parent', '--merges', '--format=%H', 'origin/main').splitlines():
    release = git('log', f'{merge}^1..{merge}^2', '--grep=^release: v', '--format=%H', '-1')
    if release:
        head = git('rev-parse', f'{release}^')
        cases.append((head, git('merge-base', f'{merge}^1', head)))
    if len(cases) == 12:
        break
assert len(cases) == 12, 'release regression requires the last twelve releases (full Git history)'
cases.append((git('rev-parse', '34f5034e5^2'), git('rev-parse', '66719836d')))
def verify(head, base, expected):
    git('update-ref', 'refs/heads/replay', head, cwd=clone)
    git('symbolic-ref', 'HEAD', 'refs/heads/replay', cwd=clone)
    shutil.rmtree(clone / 'changelog.d', ignore_errors=True)
    (clone / 'changelog.d').mkdir()
    for path in git('ls-tree', '-r', '--name-only', head, '--', 'changelog.d/', cwd=clone).splitlines():
        (clone / path).write_bytes(subprocess.check_output(['git', 'show', f'{head}:{path}'], cwd=clone))
    result = subprocess.run(['python3', str(root / 'scripts/release_changelog.py'),
                             'verify-deliveries', base], cwd=clone, text=True, capture_output=True)
    print(f'{head} expected={expected} actual={result.returncode} {result.stderr.strip()}')
    assert result.returncode == expected, result.stderr
for head, base in cases:
    verify(head, base, 0)
# Build the actual incident without the fix, only the red and notes commits.
shutil.rmtree(clone / 'changelog.d')
git('sparse-checkout', 'init', '--cone', cwd=clone)
git('sparse-checkout', 'set', 'changelog.d', 'docs/release-notes', cwd=clone)
git('checkout', '--quiet', '--force', '--detach', 'af4c35f71', cwd=clone)
git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
    'cherry-pick', 'd702e7333', '8890a7a95', cwd=clone)
verify(git('rev-parse', 'HEAD', cwd=clone), git('rev-parse', '66719836d'), 1)
print('12 release preparations + actual shipped tip accepted; incident refused')
PYHISTORY
    cat "$tmp/history.log"
    return "$status"
}
check 'historical_release_replay' 'cas-4bdf: twelve real releases and shipped peer fix pass; actual incident refuses'

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
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/web-failure
}
check 'commander_build_failure_stops_release' 'a failed Commander build stops before the release commit or push'

invalid_draft_stops_cut() {
    local fixture="$tmp/invalid-draft" before status=0
    clone_epic "$fixture" epic/invalid-draft invalid-draft
    cat >"$tmp/invalid-slack.md" <<'EOF2'
```text
*Live on production — User — Cassy v{{VERSION}}*
Was: {{INVALID_DRAFT}} was unresolved. → Now: the draft is complete before publication.
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
        grep -qF '[rule: unresolved_token]' "$tmp/invalid-draft.log" &&
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
: >"$FAKE_GH_LOG"
git -C "$epic" update-ref refs/remotes/origin/main "$(git -C "$tmp/seed" rev-parse HEAD^)"
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
check '! grep -q "^pr checks\|^run view" "$FAKE_GH_LOG"' 'cas-61e0: queue admission never waits for a second PR CI proof'
check 'grep -q "^release track --commit $(git -C "$epic" rev-parse HEAD) --version 9.100.0" "$FAKE_CAS_LOG"' 'cas-61e0: --no-watch automatically arms durable landing and publication reminders'
check 'grep -q -- "--first-child $epic_first_child --preview " "$FAKE_CAS_LOG"' 'cas-61e0: fresh integration history excludes unrelated main commits from the first-child timer'
check '! git -C "$tmp/origin.git" show-ref --tags | grep -q v9.100.0' 'nothing is tagged'

# 4. cas-3873: after an ejection repair, rerun the same command even on a
# later day. Both the original (now removed) and renamed notes paths work.
rerun_after_repair() {
    local before notes out status
    rm -f "$FAKE_GH_STATE/admitted"
    echo repaired >"$epic/alpha.txt"
    git -C "$epic" commit --quiet -am 'repair queue failure'
    before="$(git -C "$epic" rev-parse HEAD)"
    for notes in "$original_notes" docs/release-notes/2026-10-15-v9.100.0-slack.md ''; do
        : >"$FAKE_GH_LOG"
        rm -f "$FAKE_GH_STATE/admitted"
        out="$(cd "$epic" && RELEASE_DATE=2026-10-16 ./scripts/release.sh --no-watch ${notes:+--notes "$notes"} 2>&1)" || { echo "$out"; return 1; }
        [[ "$(git -C "$epic" rev-parse HEAD)" == "$before" && -z "$(git -C "$epic" status --porcelain)" ]] || return 1
        [[ "$(git -C "$tmp/origin.git" rev-parse refs/heads/epic/alpha)" == "$before" ]] || return 1
        grep -q '^pr edit 77 --title Release v9.100.0:' "$FAKE_GH_LOG" &&
            grep -q '^pr merge 77 --auto --merge' "$FAKE_GH_LOG" &&
            ! grep -q '^pr create' "$FAKE_GH_LOG" || return 1
    done
    status=0
    rm -f "$FAKE_GH_STATE/admitted"
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
rm -f "$FAKE_GH_STATE/admitted" "$FAKE_GH_STATE/pr"
run "$tmp/beta" --no-watch --notes "$tmp/beta-draft.md" 2>/dev/null
git -C "$tmp/beta" push --quiet origin epic/beta:main
rm -f "$FAKE_GH_STATE/admitted" "$FAKE_GH_STATE/pr"; echo 77 >"$FAKE_GH_STATE/pr"
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

# 7. A queued rerun is one read, no duplicate admission and no polling.
git -C "$tmp/seed" revert --no-edit HEAD >/dev/null
git -C "$tmp/seed" push --quiet origin main
run "$epic" --no-watch 2>/dev/null
: >"$FAKE_GH_LOG"
run "$epic" 2>"$tmp/event-driven.err"
check '! grep -q "^pr merge\|^pr checks" "$FAKE_GH_LOG" && [[ "$(grep -c "^api graphql" "$FAKE_GH_LOG")" == 1 ]]' 'cas-61e0: an admitted train reuses its PR and returns after one enqueue smoke check'
# Auto-merge must really be enabled; a successful command with no admission is refused.
echo '{"state":"OPEN","autoMergeRequest":null,"mergeQueueEntry":null}' >"$tmp/not-enqueued.json"
status=0
(cd "$epic" && FAKE_PR_STATE_JSON="$tmp/not-enqueued.json" ./scripts/release.sh --no-watch) >"$tmp/not-enqueued.err" 2>&1 || status=$?
check '[[ "$status" == 3 ]]' 'cas-61e0: a missing enqueue is reported immediately after admission'

# cas-d637: replay a release beside other queued PRs. Never enable admission
# when GitHub can batch the release, skip per-entry checks, or hide its config.
for config in '{"maximumEntriesToMerge":5,"mergingStrategy":"ALLGREEN"}' '{"maximumEntriesToMerge":1,"mergingStrategy":"HEADGREEN"}' null; do
    rm -f "$FAKE_GH_STATE/admitted"
    : >"$FAKE_GH_LOG"
    status=0
    (cd "$epic" && FAKE_QUEUE_CONFIG="$config" ./scripts/release.sh --no-watch) >"$tmp/unsafe-queue.err" 2>&1 || status=$?
    check '[[ "$status" == 3 ]] && ! grep -q "^pr merge" "$FAKE_GH_LOG"' "cas-d637: unsafe release queue config $config refuses before enqueue"
done

# A hotfix remains an explicit exception, including actual CLI refusal shapes.
hotfix_fallback() {
    local mode="$1" fixture="$tmp/hotfix-$1" status=0
    clone_epic "$fixture" "release/v9.99.1-$mode" "hotfix-$mode"
    mkdir -p "$tmp/state-$mode"
    write_valid_draft "$tmp/hotfix-$mode.md"
    FAKE_CONTEXT_MODE="$mode" FAKE_TRACK_EXIT="${2:-0}" FAKE_GH_STATE="$tmp/state-$mode" \
        run "$fixture" --hotfix --base main --no-watch --notes "$tmp/hotfix-$mode.md" >"$tmp/hotfix-$mode.log" 2>&1 || status=$?
    [[ "$status" != 0 ]] &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet "refs/heads/release/v9.99.1-$mode" &&
        ! git -C "$fixture" log -1 --format=%B | grep -q '^Train-CI-Head:'
}
train_receipt_cut() {
    local fixture="$tmp/train-ci-cut" status=0 before after
    clone_epic "$fixture" epic/release-train-fixture train-ci
    write_valid_draft "$tmp/train-ci-notes.md"
    before="$(git -C "$fixture" rev-parse HEAD)"
    mkdir -p "$tmp/train-ci-artifact"
    export FAKE_TRAIN_ARTIFACT="$tmp/train-ci-artifact" FAKE_TRAIN_RUN="$tmp/train-ci-run.json" FAKE_TRAIN_LOG_ZIP="$tmp/train-ci-logs.zip"
    echo '{"workflow_runs":[]}' >"$FAKE_TRAIN_RUN"
    run "$fixture" --dry-run --notes "$tmp/train-ci-notes.md" >"$tmp/train-ci-cut.log" 2>&1 || status=$?
    [[ "$status" != 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] || return 1
    python3 - "$before" "$fixture" <<'PY'
import json, os, pathlib, subprocess, sys, zipfile
head=sys.argv[1]
repo=sys.argv[2]
json.dump({'workflow_runs':[{'id':123,'run_attempt':1,'head_sha':head,'head_branch':'epic/release-train-fixture','event':'push','status':'completed','conclusion':'success'}]},open(os.environ['FAKE_TRAIN_RUN'],'w'))
jobs={name:'success' for name in ('fast-validation-preflight','fast-validation-ci-tiers','fast-validation-web','fast-validation-suite','fast-validation-docs','queue-flake-publication')}
receipt={'schema':2,'scope':'full-set','head':head,'branch':'epic/release-train-fixture','run_id':'123','run_attempt':1,'outcome':'PASS','failures':[],
         'tree':subprocess.check_output(['git','-C',repo,'rev-parse','HEAD^{tree}'],text=True).strip(),
         'main_baseline':subprocess.check_output(['git','-C',repo,'rev-parse','origin/main'],text=True).strip(),
         'required_jobs':list(jobs),'jobs':jobs,'elapsed_seconds':12,'journeys_required':False,'bridge_required':False}
json.dump(receipt,open(pathlib.Path(os.environ['FAKE_TRAIN_ARTIFACT'])/'receipt.json','w'))
with zipfile.ZipFile(os.environ['FAKE_TRAIN_LOG_ZIP'],'w') as archive: archive.writestr('full.log','Complete fixture log')
PY
    cp "$FAKE_TRAIN_ARTIFACT/receipt.json" "$tmp/train-ci-green.json"
    python3 - "$FAKE_TRAIN_ARTIFACT/receipt.json" <<'PY'
import json,sys
p=sys.argv[1]; row=json.load(open(p)); row['jobs']['fast-validation-ci-tiers']='skipped'; json.dump(row,open(p,'w'))
PY
    status=0
    run "$fixture" --dry-run --notes "$tmp/train-ci-notes.md" >>"$tmp/train-ci-cut.log" 2>&1 || status=$?
    [[ "$status" != 0 && "$(git -C "$fixture" rev-parse HEAD)" == "$before" ]] || return 1
    cp "$tmp/train-ci-green.json" "$FAKE_TRAIN_ARTIFACT/receipt.json"
    rm -rf "$fixture/.git/cas/train-ci"
    run "$fixture" --dry-run --notes "$tmp/train-ci-notes.md" >>"$tmp/train-ci-cut.log" 2>&1 || { cat "$tmp/train-ci-cut.log"; return 1; }
    after="$(git -C "$fixture" rev-parse HEAD)"
    [[ "$after" != "$before" && "$(git -C "$fixture" log -1 --format=%B)" == *"Train-CI-Head: $before"* ]] || return 1
    run "$fixture" --dry-run --notes "$tmp/train-ci-notes.md" >>"$tmp/train-ci-cut.log" 2>&1 || { cat "$tmp/train-ci-cut.log"; return 1; }
    [[ "$(git -C "$fixture" rev-parse HEAD)" == "$after" ]]
}
check 'train_receipt_cut' 'train cut requires green starting-tip receipt and reuses it on an unchanged prepared retry'
unset FAKE_TRAIN_ARTIFACT FAKE_TRAIN_RUN FAKE_TRAIN_LOG_ZIP
check 'hotfix_fallback no_train' 'a standalone hotfix with no train or included task cannot build a cut candidate'
check 'hotfix_fallback different_branch' 'an unrelated train cannot provide candidate identity for a standalone hotfix'
freeze_failure_stops_publication() {
    local fixture="$tmp/cut-failure" status=0
    clone_epic "$fixture" epic/cut-failure cut-failure
    write_valid_draft "$tmp/cut-failure.md"
    : >"$FAKE_GH_LOG"
    FAKE_TRACK_EXIT=1 run "$fixture" --no-watch --notes "$tmp/cut-failure.md" >"$tmp/cut-failure.log" 2>&1 || status=$?
    [[ "$status" -ne 0 ]] && grep -qi 'freeze.*retry\|freeze.*Retry' "$tmp/cut-failure.log" &&
        ! grep -q '^pr create\|^pr merge' "$FAKE_GH_LOG" &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/cut-failure
}
check 'freeze_failure_stops_publication' 'cas-2b8d: a matched train that cannot freeze stops before push or enqueue and names its retry'

echo '{"state":"OPEN","autoMergeRequest":{"enabledAt":"2026-11-05T23:00:00Z"},"mergeQueueEntry":null}' >"$tmp/pending-auto.json"
echo '[{"name":"required CI","bucket":"fail"}]' >"$tmp/checks-red.json"
status=0
(cd "$epic" && FAKE_PR_STATE_JSON="$tmp/pending-auto.json" FAKE_PR_CHECKS_JSON="$tmp/checks-red.json" ./scripts/release.sh --no-watch) >"$tmp/pending-red.err" 2>&1 || status=$?
check '[[ "$status" != 0 ]] && grep -qi "not admitted" "$tmp/pending-red.err" && ! grep -q "admission verified" "$tmp/pending-red.err"' 'cas-61e0: pending auto-merge with failed required checks is not admission'

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

# A one-item hotfix is an ordinary train cut, queue landing and publication.
one_item_hotfix_train() {
    local fixture="$tmp/one-item-hotfix" branch="epic/one-item-hotfix" version cut landed
    clone_epic "$fixture" "$branch" one-hotfix
    [[ "$(git -C "$fixture" rev-list --count origin/main..HEAD)" == 1 ]] || return 1
    version="$(run "$fixture" --next-version --hotfix | sed -n 's/^version=//p')"
    write_valid_draft "$tmp/one-hotfix-notes.md"
    mkdir -p "$tmp/one-hotfix-state" "$tmp/one-hotfix-assets"
    FAKE_GH_STATE="$tmp/one-hotfix-state" FAKE_GH_LOG="$tmp/one-hotfix-gh.log" \
        FAKE_CAS_LOG="$tmp/one-hotfix-cas.log" FAKE_ESTIMATE_EXIT=23 \
        run "$fixture" --hotfix --notes "$tmp/one-hotfix-notes.md" >"$tmp/one-hotfix.log" 2>&1 || { cat "$tmp/one-hotfix.log"; return 1; }
    cut="$(git -C "$fixture" rev-parse HEAD)"
    [[ "$(git -C "$fixture" log -1 --format=%s)" == "release: v$version" &&
       "$(git -C "$tmp/origin.git" rev-parse "refs/heads/$branch")" == "$cut" ]] || return 1
    grep -q "^pr create --base main --head $branch " "$tmp/one-hotfix-gh.log" &&
        [[ "$(grep -c '^pr merge ' "$tmp/one-hotfix-gh.log")" == 1 ]] &&
        grep -q "^release track --commit $cut --version $version " "$tmp/one-hotfix-cas.log" || return 1
    grep -q "^ledger estimate release v$version$" "$tmp/one-hotfix-cas.log" || return 1
    [[ "$(grep -c '^ledger estimate pipeline ' "$tmp/one-hotfix-cas.log")" == 3 ]] || return 1
    # Stand in only for the external merge queue's landing, then invoke the
    # same publisher as the release workflow against its exact landed SHA.
    git -C "$fixture" switch --quiet main
    git -C "$fixture" merge --quiet --no-ff -m 'queue landing' "$branch"
    git -C "$fixture" push --quiet origin main
    landed="$(git -C "$fixture" rev-parse HEAD)"
    printf 'fixture linux archive\n' >"$tmp/one-hotfix-assets/cas-x86_64-unknown-linux-gnu.tar.gz"
    printf 'fixture macos archive\n' >"$tmp/one-hotfix-assets/cas-aarch64-apple-darwin.tar.gz"
    (cd "$fixture" && GITHUB_REPOSITORY=example/repo FAKE_GH_LOG="$tmp/one-hotfix-gh.log" \
        "$script_dir/release-publish.sh" --version "$version" --sha "$landed" \
        --assets "$tmp/one-hotfix-assets" --notes-out "$tmp/one-hotfix-published.md") >>"$tmp/one-hotfix.log" 2>&1 || { cat "$tmp/one-hotfix.log"; return 1; }
    [[ "$(git -C "$tmp/origin.git" rev-parse "refs/tags/v$version^{}")" == "$landed" ]] &&
        grep -q "^release create v$version " "$tmp/one-hotfix-gh.log" &&
        grep -q 'one-hotfix shipped' "$tmp/one-hotfix-published.md"
}
check 'one_item_hotfix_train' 'cas-d725: one-item hotfix train cuts, queues once and publishes through the normal release path'

cut_preview_gate() {
    local fixture="$tmp/preview-cut" status=0
    clone_epic "$fixture" epic/preview-cut preview-cut
    write_valid_draft "$tmp/preview-notes.md"
    : >"$FAKE_CAS_LOG"
    : >"$FAKE_GH_LOG"
    FAKE_WOULD_CLOSE=0 run "$fixture" --hotfix --notes "$tmp/preview-notes.md" >"$tmp/preview-cut.log" 2>&1 || status=$?
    [[ "$status" != 0 ]] &&
        ! git -C "$tmp/origin.git" show-ref --verify --quiet refs/heads/epic/preview-cut &&
        ! grep -q '^release track ' "$FAKE_CAS_LOG" &&
        ! grep -q '^pr merge ' "$FAKE_GH_LOG" || return 1
    : >"$FAKE_CAS_LOG"
    FAKE_WOULD_CLOSE=1 run "$fixture" --hotfix --notes "$tmp/preview-notes.md" >>"$tmp/preview-cut.log" 2>&1 || return 1
    [[ "$(git -C "$tmp/origin.git" rev-parse refs/heads/epic/preview-cut)" == "$(git -C "$fixture" rev-parse HEAD)" ]] &&
        [[ "$(grep -c '^qa candidate build ' "$FAKE_CAS_LOG")" == 1 ]] &&
        grep -q '^release preview --commit ' "$FAKE_CAS_LOG" &&
        grep -q '^release track .* --preview ' "$FAKE_CAS_LOG"
}
check 'cut_preview_gate' 'cut refuses zero would-close before push and permits approved members with one candidate build'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
