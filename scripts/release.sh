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
#      [Unreleased] and the changelog.d fragments to a dated [x.y.z]
#      section, and names the release-notes draft
#      docs/release-notes/<date>-v<x.y.z>-slack.md, then rebuilds the
#      embedded Commander bundle with the new version;
#   4. commits "release: v<x.y.z>", pushes, opens/reuses the train PR,
#      admits it once and verifies admission with one smoke check.
# Durable cross-session SHA/tag reminders carry landing and publication;
# this command returns after admission, including with --no-watch.
#
# Landing on main is the release: the merge queue's run builds the archives
# (release-prebuild.yml) and release.yml publishes them. Nothing here tags,
# builds Rust or publishes. The local train and gate scripts are not called.
#
# The draft: write the Slack draft before running, at the path printed by
# --next-version, or anywhere with --notes <file>; `{{VERSION}}` in it becomes
# the release version. A re-bump renames it and rewrites its version.
#
# Exit codes: 0 prepared and admitted (or dry run); 1 setup error;
# 3 no queue admission after the enqueue request; 4 non-release merge conflict.

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
  --no-watch          compatible alias; reminders are armed automatically
  --dry-run           prepare and commit locally; no push, no PR
EOF
}

base="main"
kind="minor"
explicit_version=""
notes_file=""
print_only=false
dry_run=false
while (($#)); do
    case "$1" in
        --hotfix) kind="patch"; shift ;;
        --version) explicit_version="${2:?--version needs x.y.z}"; shift 2 ;;
        --base) base="${2:?--base needs a branch}"; shift 2 ;;
        --notes) notes_file="${2:?--notes needs a file}"; shift 2 ;;
        --next-version) print_only=true; shift ;;
        --no-watch) shift ;;
        --dry-run) dry_run=true; shift ;;
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
cas_bin="${CAS_BIN:-cas}"
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
                if git cat-file -e "$remote/$base:$file" 2>/dev/null; then
                    git restore --source="$remote/$base" --staged --worktree -- "$file"
                else
                    git rm --quiet -- "$file"
                    continue # The deletion is staged; no path remains to add.
                fi
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

lint_release_draft() {
    local draft="$1"
    python3 "$script_dir/release-train-announce.py" --lint "$draft" >&2 \
        || die "release-notes draft failed lint: $draft"
}

# Steps 1-3 plus the commit. Prints the version prepared.
prepare() {
    local base_version version current prepared_commit drafts=() prepared_drafts=()
    fetch_base
    sync_with_base
    python3 "$script_dir/release_changelog.py" verify-deliveries "$remote/$base"
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
        mapfile -t prepared_drafts < <(git ls-files "docs/release-notes/*-v$version-slack.md")
        if ((${#prepared_drafts[@]} == 1)); then
            lint_release_draft "${prepared_drafts[0]}"
        else
            lint_release_draft "$(draft_path "$version")"
        fi
        say "v$version is already prepared on $branch"
        printf '%s\n' "$version"
        return 0
    fi
    local source
    source="$(draft_source "$version")"
    lint_release_draft "$source"
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
    # Promotion deleted the changelog.d fragments it folded in (cas-f2be).
    if git ls-files --error-unmatch changelog.d >/dev/null 2>&1; then
        git add -- changelog.d
    fi
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
    local state
    state="$(pr_state)"
    if ! admitted <<<"$state"; then
        "$gh_bin" pr merge "$pr_number" --auto --merge >/dev/null
        state="$(pr_state)"
    fi
    admitted <<<"$state" || {
        printf 'error: PR #%s has no auto-merge and no merge-queue entry after enqueue; repair admission and rerun.\n' "$pr_number" >&2
        exit 3
    }
    say "PR #$pr_number admission verified; the merge queue proves this tree"
}

# One enqueue smoke check, never a watch or CI polling loop.
admitted() {
    jq -e '.state == "MERGED" or .autoMergeRequest != null or .mergeQueueEntry != null' >/dev/null
}
pr_state() {
    "$gh_bin" api graphql -F owner="${repo_slug%%/*}" -F name="${repo_slug#*/}" -F number="$pr_number" -f query='
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      state autoMergeRequest { enabledAt } mergeQueueEntry { state }
    }
  }
}' --jq '.data.repository.pullRequest'
}

publish_branch() {
    git push --quiet "$remote" "HEAD:refs/heads/$branch"
}

# Advisory once per invocation, including dry-run and version preview. Never
# include this probe in prepare/rebump loops or fail a release on provider drift.
python3 "$script_dir/github_queue_health.py" --worktree "$repo_root" --gh "$gh_bin" \
    || say "warning: merge queue unavailable; report only"

if "$print_only"; then
    fetch_base
    base_version="$(version_at "$remote/$base")"
    version="$(next_version "$base_version")"
    printf 'version=%s\n' "$version"
    printf 'draft=%s\n' "$(draft_path "$version")"
    exit 0
fi

[[ -z "$(git status --porcelain --untracked-files=no)" ]] || die "the worktree has uncommitted changes; commit or stash them first"

# Resolve through Cassy's canonical train helper before changing release files.
# The CLI owns train selection/owner/base; the script only verifies its checkout.
context="$("$cas_bin" release context --branch "$branch")"
[[ "$(jq -r .branch <<<"$context")" == "$branch" ]] || die "check out the open release train branch first"
base="$(jq -er .integration <<<"$context")"
train_base="$(jq -er .base_commit <<<"$context")"
first_child="$(git log --format=%H --no-merges "$train_base..HEAD" | tail -1)"
version="$(prepare)"
if "$dry_run"; then
    say "dry run: v$version committed locally on $branch; nothing pushed"
    exit 0
fi
# Persist context and arm reminders before push: publication may follow quickly,
# and a retry after any later failure must reuse the same reminder occurrence.
"$cas_bin" release track --commit "$(git rev-parse HEAD)" --version "$version" --first-child "$first_child"
publish_branch
repo_slug="$("$gh_bin" repo view --json nameWithOwner -q .nameWithOwner)"
open_or_update_pr "$version"
say "v$version is prepared; durable landing and publication reminders are armed"
