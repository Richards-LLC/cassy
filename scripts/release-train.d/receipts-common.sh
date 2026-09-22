#!/usr/bin/env bash

release_train_receipts_record_field() {
    local record="$1" key="$2"
    sed -n "s/^${key}=//p" "$record" | head -n1
}

release_train_receipts_record_files() {
    find "$artifacts_root" -mindepth 2 -maxdepth 2 -type f \
        -name receipts.commit -print 2>/dev/null | sort
}

release_train_receipts_unmerged_records() {
    local record commit branch
    while IFS= read -r record; do
        [[ -n "$record" && "$record" != "$run_dir/receipts.commit" ]] || continue
        commit="$(release_train_receipts_record_field "$record" COMMIT_SHA)"
        [[ "$commit" =~ ^[0-9a-f]{40}$ ]] || continue
        if ! git -C "$worktree" merge-base --is-ancestor "$commit" HEAD 2>/dev/null; then
            branch="$(release_train_receipts_record_field "$record" BRANCH)"
            printf '%s\t%s\t%s\n' "$record" "$commit" "$branch"
        fi
    done < <(release_train_receipts_record_files)
}

release_train_receipts_carry_pending() {
    local record commit branch target
    while IFS=$'\t' read -r record commit branch; do
        [[ -n "$record" ]] || continue
        if [[ -n "$branch" ]]; then
            if ! git -C "$worktree" fetch -q --no-tags origin \
                "refs/heads/$branch:refs/remotes/origin/$branch"; then
                printf 'ERROR prep receipts: could not fetch recorded branch %s for %s\n' \
                    "$branch" "$commit" >&2
                return 1
            fi
            target="refs/remotes/origin/$branch"
            if ! git -C "$worktree" merge-base --is-ancestor "$commit" "$target" 2>/dev/null; then
                printf 'ERROR prep receipts: branch %s does not contain recorded commit %s\n' \
                    "$branch" "$commit" >&2
                return 1
            fi
        else
            target="$commit"
            git -C "$worktree" cat-file -e "$target^{commit}" 2>/dev/null || {
                printf 'ERROR prep receipts: recorded commit %s is unavailable locally\n' "$commit" >&2
                return 1
            }
        fi
        if ! git -C "$worktree" -c core.hooksPath=/dev/null merge --no-edit "$target" >/dev/null; then
            printf 'ERROR prep receipts: could not merge recorded commit %s from %s\n' \
                "$commit" "${branch:-receipt}" >&2
            return 1
        fi
        printf 'prep carried receipts commit %s from %s\n' "$commit" "${branch:-receipt}"
    done < <(release_train_receipts_unmerged_records)
}
