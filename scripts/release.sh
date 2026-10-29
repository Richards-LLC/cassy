#!/usr/bin/env bash
#
# One-command release for CAS (cas-f95b). Run it from the epic branch (or a
# hotfix branch) once the work is code-done:
#
#   scripts/release.sh                 # next minor version
#   scripts/release.sh --hotfix        # next patch version
#   scripts/release.sh --next-version  # print the version it would use; no change
#
# It commits the release onto the branch itself, so the version reaches main
# through the same PR and the same merge-queue run as the code:
#   1. merges the base branch in when the branch is behind it, resolving the
#      release files (version lines, Cargo.lock, CHANGELOG, draft) itself;
#   2. computes the next version from the base branch's version;
#   3. bumps the release crates and Cargo.lock, promotes CHANGELOG
#      [Unreleased] to a dated [x.y.z] section, and names the release-notes
#      draft docs/release-notes/<date>-v<x.y.z>-slack.md, then rebuilds the
#      embedded Commander bundle with the new version;
#   4. commits "release: v<x.y.z>", pushes, opens the PR to the base branch
#      (or retitles the open one), waits for the PR's one CI run (Scoped
#      Validation) to go green, and only then enables auto-merge (cas-2aef).
# Then it watches the PR until it merges. When another release lands on the
# base first, it re-bumps: merges the base, takes the next version, pushes
# again and re-enables auto-merge. It never waits for a person on that path.
#
# Landing on main is the release: the merge queue's run builds the archives
# (release-prebuild.yml) and release.yml publishes them. Nothing here tags,
# builds Rust or publishes. The local train and gate scripts are not called.
#
# The draft: write the Slack draft before running, at the path printed by
# --next-version, or anywhere with --notes <file>; `{{VERSION}}` in it becomes
# the release version. A re-bump renames it and rewrites its version.
#
# Exit codes: 0 merged (or prepared with --no-watch); 1 usage or setup error;
# 3 the merge queue ejected the PR or auto-merge was turned off (repair, then
# rerun this command); 4 a merge conflict outside the release files; 5 the
# PR's CI is red, reported with its failing tests (it never entered the queue).

set -Eeuo pipefail
# prepare runs in a command substitution; keep errexit there too.
shopt -s inherit_errexit
# Include commands inside prepare's substitution: Git and gh can fail without
# printing anything (notably a quiet commit with nothing to commit).
trap 'printf "error: command failed (exit %s) at line %s: %s\n" "$?" "$LINENO" "$BASH_COMMAND" >&2' ERR

usage() {
    sed -n '3,33p' "$0" | sed 's/^# \{0,1\}//'
    cat <<'EOF'
Options:
  --hotfix            bump the patch version instead of the minor version
  --version <x.y.z>   use this version (must be above the base branch's)
  --base <branch>     base branch (default main)
  --notes <file>      release-notes draft to commit (default: the draft this
                      branch adds under docs/release-notes/)
  --next-version      print the version and draft path, change nothing
  --no-watch          stop after enabling auto-merge
  --dry-run           prepare and commit locally; no push, no PR
EOF
}

base="main"
kind="minor"
explicit_version=""
notes_file=""
print_only=false
watch=true
dry_run=false
while (($#)); do
    case "$1" in
        --hotfix) kind="patch"; shift ;;
        --version) explicit_version="${2:?--version needs x.y.z}"; shift 2 ;;
        --base) base="${2:?--base needs a branch}"; shift 2 ;;
        --notes) notes_file="${2:?--notes needs a file}"; shift 2 ;;
        --next-version) print_only=true; shift ;;
        --no-watch) watch=false; shift ;;
        --dry-run) dry_run=true; watch=false; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'error: unknown argument %s\n' "$1" >&2; usage >&2; exit 1 ;;
    esac
done

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
# Progress goes to stderr: prepare's stdout is the version it prepared.
say() { printf 'release: %s\n' "$*" >&2; }

if [[ -n "$notes_file" && "$notes_file" != /* ]]; then
    # Keep the caller's directory, but resolve a missing path after finding the
    # release commit: the first run may have renamed this very draft.
    notes_file="$PWD/$notes_file"
fi

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"
script_dir="$repo_root/scripts"
remote="${RELEASE_REMOTE:-origin}"
gh_bin="${GH_BIN:-gh}"
watch_interval="${RELEASE_WATCH_INTERVAL:-60}"
release_date() { printf '%s\n' "${RELEASE_DATE:-$(date -u +%Y-%m-%d)}"; }

release_manifests=(
    cas-cli/Cargo.toml
    crates/cas-types/Cargo.toml
    crates/cas-search/Cargo.toml
    crates/cas-store/Cargo.toml
    crates/cas-core/Cargo.toml
    crates/cas-mcp/Cargo.toml
)
release_packages='cas|cas-types|cas-search|cas-store|cas-core|cas-mcp'

[[ -z "$explicit_version" || "$explicit_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "--version must be x.y.z, got $explicit_version"

branch="$(git symbolic-ref --quiet --short HEAD)" || die "detached HEAD; check out the epic or hotfix branch"
[[ "$branch" != "$base" ]] || die "run this on the epic or hotfix branch, not on $base"

version_at() {
    git show "$1:cas-cli/Cargo.toml" 2>/dev/null | awk -F'"' '/^version = "/ { print $2; exit }' || true
}

remote_tag_exists() {
    git ls-remote --exit-code --tags "$remote" "refs/tags/v$1" >/dev/null 2>&1
}

bump() {
    local version="$1" major minor patch
    IFS=. read -r major minor patch <<<"$version"
    case "$kind" in
        patch) printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))" ;;
        *) printf '%s.%s.0\n' "$major" "$((minor + 1))" ;;
    esac
}

version_greater() {
    [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" == "$1" ]]
}

# The version this release takes: one step above the base branch, skipping a
# version whose tag already exists.
next_version() {
    local base_version="$1" candidate
    if [[ -n "$explicit_version" ]]; then
        version_greater "$explicit_version" "$base_version" \
            || die "--version $explicit_version is not above $base's $base_version"
        candidate="$explicit_version"
    else
        candidate="$(bump "$base_version")"
    fi
    while remote_tag_exists "$candidate"; do
        candidate="$(bump "$candidate")"
    done
    printf '%s\n' "$candidate"
}

draft_path() { printf 'docs/release-notes/%s-v%s-slack.md\n' "$(release_date)" "$1"; }

fetch_base() {
    git fetch --quiet "$remote" "+refs/heads/$base:refs/remotes/$remote/$base" \
        || die "could not fetch $remote/$base"
}

# Drafts this branch adds relative to the base branch.
branch_drafts() {
    git diff --name-only --diff-filter=A "$remote/$base...HEAD" -- 'docs/release-notes/*-v*-slack.md'
}

our_conflict_draft=""

# Merge the base branch in. Conflicts in the release files resolve
# mechanically; any other conflict aborts the merge and stops (exit 4).
sync_with_base() {
    local merge_base merge_base_version file conflicted=() other=() scratch
    if git merge-base --is-ancestor "$remote/$base" HEAD; then
        return 0
    fi
    merge_base="$(git merge-base HEAD "$remote/$base")"
    merge_base_version="$(version_at "$merge_base")"
    scratch="$(mktemp -d)"
    git show HEAD:CHANGELOG.md >"$scratch/ours-changelog.md"
    say "merging $remote/$base into $branch"
    if git -c core.hooksPath=/dev/null merge --no-edit --no-ff "$remote/$base" >"$scratch/merge.log" 2>&1; then
        rm -rf "$scratch"
        return 0
    fi
    mapfile -t conflicted < <(git diff --name-only --diff-filter=U)
    for file in "${conflicted[@]}"; do
        case "$file" in
            CHANGELOG.md|Cargo.lock|cas-cli/Cargo.toml|crates/cas-*/Cargo.toml|docs/release-notes/*-slack.md|hub-web/dist/*) ;;
            *) other+=("$file") ;;
        esac
    done
    if ((${#other[@]})); then
        git merge --abort
        rm -rf "$scratch"
        printf 'error: merging %s conflicts outside the release files:\n' "$remote/$base" >&2
        printf '  %s\n' "${other[@]}" >&2
        printf 'Resolve the merge on %s by hand, push, then rerun scripts/release.sh.\n' "$branch" >&2
        exit 4
    fi
    for file in "${conflicted[@]}"; do
        case "$file" in
            hub-web/dist/*)
                # Generated files may conflict when two releases change web
                # inputs. Take the base output (including deletions); prepare
                # regenerates it from the merged sources and bumped version.
                git restore --source="$remote/$base" --staged --worktree -- "$file"
                ;;
            CHANGELOG.md)
                git show ":3:$file" >"$scratch/base-changelog.md"
                python3 "$script_dir/release_changelog.py" rebase "$scratch/ours-changelog.md" \
                    "$scratch/base-changelog.md" "$merge_base_version" "$file"
                ;;
            docs/release-notes/*)
                # Both sides added this path: theirs is a published draft.
                our_conflict_draft="$scratch/$(basename "$file")"
                git show ":2:$file" >"$our_conflict_draft"
                git checkout --theirs -- "$file"
                ;;
            *)
                # Version lines and lock entries: take the base side of each
                # conflicting hunk only; the bump below rewrites the versions.
                git show ":2:$file" >"$scratch/ours"
                git show ":1:$file" >"$scratch/common" 2>/dev/null || : >"$scratch/common"
                git show ":3:$file" >"$scratch/theirs"
                git merge-file --theirs -q "$scratch/ours" "$scratch/common" "$scratch/theirs" || true
                cp "$scratch/ours" "$file"
                ;;
        esac
        git add -- "$file"
    done
    git -c core.hooksPath=/dev/null commit --quiet --no-edit
    say "merged $remote/$base; resolved release files: ${conflicted[*]}"
}

set_versions() {
    local version="$1" manifest
    for manifest in "${release_manifests[@]}"; do
        [[ -f "$manifest" ]] || die "missing $manifest"
        perl -0777 -i -pe "s/^version = \"[^\"]*\"/version = \"$version\"/m" "$manifest"
    done
    perl -0777 -i -pe \
        "s/(\\[\\[package\\]\\]\\nname = \"(?:$release_packages)\"\\nversion = \")[^\"]*\"/\${1}$version\"/g" \
        Cargo.lock
}

# The draft this release commits: --notes, our side of a draft path the base
# also added, or the one draft this branch adds. Checked before any edit.
draft_source() {
    local version="$1" drafts=()
    if [[ -n "$notes_file" ]]; then
        printf '%s\n' "$notes_file"
    elif [[ -n "$our_conflict_draft" ]]; then
        printf '%s\n' "$our_conflict_draft"
    else
        mapfile -t drafts < <(branch_drafts)
        if ((${#drafts[@]} > 1)); then
            die "this branch adds ${#drafts[@]} release-notes drafts (${drafts[*]}); keep one or pass --notes"
        fi
        if ((${#drafts[@]} == 0)); then
            die "no release-notes draft: write $(draft_path "$version") (follow docs/release-notes/RUBRIC.md; {{VERSION}} is replaced), or pass --notes <file>"
        fi
        printf '%s\n' "${drafts[0]}"
    fi
}

# Name the draft for this version and rewrite its version text.
place_draft() {
    local version="$1" source="$2" target old_version
    target="$(draft_path "$version")"
    old_version="$(basename "$source" | sed -n 's/.*-v\([0-9]*\.[0-9]*\.[0-9]*\)-slack\.md$/\1/p')"
    mkdir -p "$(dirname "$target")"
    if [[ "$source" != "$target" ]]; then
        cp "$source" "$target.new"
        if git ls-files --error-unmatch "$source" >/dev/null 2>&1; then
            git rm --quiet -- "$source"
        fi
        mv "$target.new" "$target"
    fi
    perl -pi -e "s/\\{\\{VERSION\\}\\}/$version/g" "$target"
    if [[ -n "$old_version" && "$old_version" != "$version" ]]; then
        perl -pi -e "s/\\Q$old_version\\E/$version/g" "$target"
    fi
    perl -pi -e "s/^(Release date: )\\d{4}-\\d{2}-\\d{2}/\${1}$(release_date)/" "$target"
    git add -- "$target"
}

# Steps 1-3 plus the commit. Prints the version prepared.
prepare() {
    local base_version version current prepared_commit drafts=()
    fetch_base
    sync_with_base
    base_version="$(version_at "$remote/$base")"
    [[ "$base_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "cannot read $base's version"
    version="$(next_version "$base_version")"
    current="$(version_at HEAD)"
    prepared_commit="$(git log -1 --format=%H --grep="^release: v${version//./[.]}$" "$remote/$base..HEAD")"
    if [[ -n "$notes_file" && ! -f "$notes_file" ]]; then
        # Only accept a vanished --notes path if this release actually removed
        # it. An unrelated typo must still fail instead of selecting any draft.
        if [[ -n "$prepared_commit" && -n "$(git diff --no-renames --name-only --diff-filter=D "$prepared_commit^" "$prepared_commit" -- "$notes_file")" ]]; then
            mapfile -t drafts < <(git diff --no-renames --name-only --diff-filter=A "$prepared_commit^" "$prepared_commit" -- "docs/release-notes/*-v$version-slack.md")
            if ((${#drafts[@]} == 1)); then
                say "--notes $notes_file was renamed to ${drafts[0]}"
                notes_file="${drafts[0]}"
            fi
        fi
        [[ -f "$notes_file" ]] || die "--notes file not found: $notes_file"
    fi
    if [[ -n "$prepared_commit" && "$current" == "$version" ]] && grep -q "^## \[$version\] - " CHANGELOG.md \
        && [[ -f "$(draft_path "$version")" || -n "$(git ls-files "docs/release-notes/*-v$version-slack.md")" ]]; then
        say "v$version is already prepared on $branch"
        printf '%s\n' "$version"
        return 0
    fi
    local source
    source="$(draft_source "$version")"
    set_versions "$version"
    # Vite embeds cas-cli's version. Build after the bump, before committing,
    # so the merge queue sees exactly the bundle a fresh build will produce.
    # Keep npm output off prepare's stdout, which carries only the version.
    say "rebuilding Commander for v$version"
    (cd hub-web && npm ci && npm run build) >&2 \
        || die "Commander build failed; release was not committed"
    python3 "$script_dir/release_changelog.py" promote CHANGELOG.md "$version" "$(release_date)" "$base_version" \
        || die "CHANGELOG.md promotion failed"
    place_draft "$version" "$source"
    git add -- CHANGELOG.md Cargo.lock "${release_manifests[@]}" hub-web/dist
    git -c core.hooksPath=/dev/null commit --quiet -m "release: v$version" \
        -m "Release-Version: $version" -m "Prepared by scripts/release.sh on $branch from $base $base_version."
    say "committed release v$version on $branch ($(git rev-parse --short HEAD))"
    printf '%s\n' "$version"
}

pr_number=""

open_or_update_pr() {
    local version="$1" title body
    title="Release v$version: $branch"
    body="$(mktemp)"
    {
        printf 'Release v%s, prepared by `scripts/release.sh`.\n\n' "$version"
        printf 'The version bump, CHANGELOG promotion and release-notes draft ride in this PR, so the merge queue proves the tree that ships. The queue run builds both release archives (Release Prebuild); landing on %s publishes them (Release). No release PR follows.\n\n' "$base"
        printf '## Changelog\n\n'
        python3 "$script_dir/release_changelog.py" section CHANGELOG.md "$version"
    } >"$body"
    pr_number="$("$gh_bin" pr list --head "$branch" --base "$base" --state open --json number --jq '.[0].number // empty')"
    if [[ -z "$pr_number" ]]; then
        "$gh_bin" pr create --base "$base" --head "$branch" --title "$title" --body-file "$body" >/dev/null
        pr_number="$("$gh_bin" pr list --head "$branch" --base "$base" --state open --json number --jq '.[0].number // empty')"
        [[ -n "$pr_number" ]] || die "opened a PR for $branch but cannot find it"
        say "opened PR #$pr_number"
    else
        "$gh_bin" pr edit "$pr_number" --title "$title" --body-file "$body" >/dev/null
        say "updated PR #$pr_number to v$version"
    fi
    "$gh_bin" pr edit "$pr_number" --add-label release >/dev/null 2>&1 || true
    rm -f "$body"
    wait_for_pr_ci
    "$gh_bin" pr merge "$pr_number" --auto --merge >/dev/null
    say "auto-merge enabled on PR #$pr_number; the merge queue takes it when admission is green"
}

# cas-2aef: the queue's 30 s admission check passed a tree whose real CI failed
# 15 minutes later, wasting the queue cycle. The PR's one CI run (Scoped
# Validation) must be green before auto-merge; a red run is reported once,
# with its failing tests, and the PR stays out of the queue.
wait_for_pr_ci() {
    local checks failed names tests
    while :; do
        checks="$("$gh_bin" pr checks "$pr_number" --json name,bucket,link 2>/dev/null || true)"
        jq -e '[.[] | select(.name | startswith("Scoped Validation"))] | length > 0 and all(.bucket != "pending")' \
            <<<"${checks:-[]}" >/dev/null 2>&1 && break
        sleep "$watch_interval"
    done
    failed="$(jq -r '.[] | select((.name | startswith("Scoped Validation")) and (.bucket == "fail" or .bucket == "cancel")) | "\(.name)\t\(.link)"' <<<"$checks")"
    if [[ -z "$failed" ]]; then
        say "PR #$pr_number CI is green"
        return 0
    fi
    names="$(cut -f1 <<<"$failed" | paste -sd ',' - | sed 's/,/, /g')"
    tests="$(cut -f2 <<<"$failed" | while read -r link; do
        "$gh_bin" run view --job "${link##*/job/}" --log-failed 2>/dev/null || true
    done | failing_tests)"
    printf 'error: PR #%s CI is red (%s), so it was not added to the merge queue.\nFailing tests: %s\nFix them, push, then rerun scripts/release.sh.\n' \
        "$pr_number" "$names" "${tests:-none named in the log; open the run}" >&2
    exit 5
}

# Failing test names from CI logs: nextest, cargo test, Python unittest and
# the scripts' own "FAIL <name>" lines, de-duplicated, at most 20.
failing_tests() {
    sed -E $'s/^([^\t]*\t){2}//; s/^[0-9-]+T[0-9:.]+Z //' \
        | sed -nE 's/^ *FAIL \[ *[0-9.]+s\] +(.*)$/\1/p; s/^test (.+) \.\.\. FAILED$/\1/p; s/^(FAIL|ERROR): (.+)$/\2/p; s/^FAIL (.+)$/\1/p' \
        | awk '!seen[$0]++' | head -20 | paste -sd ';' - | sed 's/;/; /g'
}

publish_branch() {
    git push --quiet "$remote" "HEAD:refs/heads/$branch"
}

if "$print_only"; then
    fetch_base
    base_version="$(version_at "$remote/$base")"
    version="$(next_version "$base_version")"
    printf 'version=%s\n' "$version"
    printf 'draft=%s\n' "$(draft_path "$version")"
    exit 0
fi

[[ -z "$(git status --porcelain --untracked-files=no)" ]] || die "the worktree has uncommitted changes; commit or stash them first"

version="$(prepare)"
if "$dry_run"; then
    say "dry run: v$version committed locally on $branch; nothing pushed"
    exit 0
fi
publish_branch
open_or_update_pr "$version"

"$watch" || exit 0

# Watch until the PR merges. Re-bump when another release takes the version.
# A queued PR has a merge-queue entry and no auto-merge request; an open PR
# with neither was ejected (or had auto-merge turned off): that needs repair.
repo_slug="$("$gh_bin" repo view --json nameWithOwner -q .nameWithOwner)"
pr_state() {
    "$gh_bin" api graphql -F owner="${repo_slug%%/*}" -F name="${repo_slug#*/}" -F number="$pr_number" -f query='
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      state mergeable
      autoMergeRequest { enabledAt }
      mergeQueueEntry { state }
      mergeCommit { oid }
    }
  }
}' --jq '.data.repository.pullRequest'
}
rebump() {
    explicit_version=""
    notes_file=""
    version="$(prepare)"
    publish_branch
    open_or_update_pr "$version"
}

start_base_version="$(version_at "$remote/$base")"
while :; do
    sleep "$watch_interval"
    state_json="$(pr_state 2>/dev/null)" || continue
    case "$(jq -r '.state' <<<"$state_json")" in
        MERGED)
            say "PR #$pr_number merged as $(jq -r '.mergeCommit.oid // "?"' <<<"$state_json"); release.yml publishes v$version"
            exit 0
            ;;
        CLOSED)
            die "PR #$pr_number was closed without merging"
            ;;
    esac
    fetch_base
    base_version="$(version_at "$remote/$base")"
    if [[ "$base_version" != "$start_base_version" ]] || remote_tag_exists "$version"; then
        say "$base moved to $base_version; re-bumping"
        rebump
        start_base_version="$base_version"
        continue
    fi
    if [[ "$(jq -r '.mergeable' <<<"$state_json")" == CONFLICTING ]]; then
        say "PR #$pr_number conflicts with $base; merging it in"
        rebump
        continue
    fi
    if [[ "$(jq -r 'if .autoMergeRequest == null and .mergeQueueEntry == null then "stalled" else "moving" end' <<<"$state_json")" == stalled ]]; then
        printf 'error: PR #%s is open with no auto-merge and no merge-queue entry: the queue ejected it or auto-merge was turned off.\n' "$pr_number" >&2
        printf 'Repair the failure, push, then rerun scripts/release.sh.\n' >&2
        exit 3
    fi
done
