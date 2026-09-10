#!/usr/bin/env bash
#
# Check whether one scoped-test invocation covers the committed source and
# integration-test surfaces it is being offered as proof for. This intentionally
# does not run cargo: run-scoped-tests.sh owns execution and calls this only
# after Cargo has reported a real, nonzero test count.

set -euo pipefail

usage() {
    echo "usage: $0 [--base <git-ref>] -- <cargo/nextest arguments>" >&2
    exit 2
}

base_ref=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || usage
            base_ref="$2"
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *) usage ;;
    esac
done

[[ $# -gt 0 ]] || usage
requested_args=("$@")

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
    echo "SCOPED PROOF SURFACE: cannot inspect the committed diff outside a Git repository." >&2
    exit 2
}
cd "$repo_root"

if [[ -z "$base_ref" ]]; then
    if git rev-parse --verify --quiet origin/main >/dev/null; then
        base_ref="origin/main"
    elif git rev-parse --verify --quiet main >/dev/null; then
        base_ref="main"
    else
        base_ref="HEAD^"
    fi
fi

merge_base="$(git merge-base "$base_ref" HEAD 2>/dev/null)" || {
    echo "SCOPED PROOF SURFACE: cannot find a merge-base between '$base_ref' and HEAD." >&2
    exit 2
}

lib_requested=false
test_targets=()
filters=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --lib)
            lib_requested=true
            ;;
        --test)
            [[ $# -ge 2 ]] || usage
            test_targets+=("$2")
            shift
            ;;
        -p|--package|-E|--filter-expr)
            [[ $# -ge 2 ]] || usage
            shift
            ;;
        --*)
            ;;
        *)
            filters+=("$1")
            ;;
    esac
    shift
done

contains_exact() {
    local wanted="$1" value
    shift
    for value in "$@"; do
        [[ "$value" == "$wanted" ]] && return 0
    done
    return 1
}

integration_target_for() {
    local nested_path="$1" directory candidate
    directory="${nested_path%%/*}"

    # GH #778: the nested path is often owned by the conventional top-level
    # integration target with the same stem (for example,
    # mcp_tools_test/task_tools/operations.rs belongs to mcp_tools_test.rs).
    # Resolve that filename before inspecting file contents so a fixture that
    # merely mentions the path cannot claim ownership.
    candidate="cas-cli/tests/${directory}.rs"
    if [[ -f "${candidate}" ]]; then
        basename "${candidate%.rs}"
        return 0
    fi

    # Some integration targets keep their root in a differently named file
    # and declare the nested module with `mod <directory>;` or an explicit
    # Rust path attribute. Only anchored declarations are ownership evidence;
    # arbitrary strings and comments are not.
    for candidate in cas-cli/tests/*.rs; do
        [[ -f "$candidate" ]] || continue
        if grep -Eq \
            "^[[:space:]]*mod[[:space:]]+${directory}[[:space:]]*;[[:space:]]*$" \
            "$candidate" \
            || grep -Eq \
            "^[[:space:]]*#\\[path[[:space:]]*=[[:space:]]*\"${directory}/[^\"[:space:]]+\"[[:space:]]*\\][[:space:]]*$" \
            "$candidate"; then
            basename "${candidate%.rs}"
            return 0
        fi
    done
    # An unfamiliar nested layout is still named loudly rather than skipped.
    printf '%s\n' "$directory"
}

add_required_test_target() {
    local target="$1" known
    for known in "${required_test_targets[@]}"; do
        [[ "$known" == "$target" ]] && return 0
    done
    required_test_targets+=("$target")
}

is_builtin_skill_or_agent_path() {
    local path="$1"
    [[ "$path" == cas-cli/src/builtins/skills/* \
        || "$path" == cas-cli/src/builtins/*/skills/* \
        || "$path" == cas-cli/src/builtins/agents/* \
        || "$path" == cas-cli/src/builtins/*/agents/* ]]
}

test_target_for_path() {
    local test_path="$1"
    case "$test_path" in
        cas-cli/tests/*/*.rs)
            integration_target_for "${test_path#cas-cli/tests/}"
            ;;
        cas-cli/tests/*.rs)
            basename "${test_path%.rs}"
            ;;
    esac
}

source_module_name_for() {
    local path="${1#cas-cli/src/}"
    path="${path%.rs}"
    if [[ "$path" == */mod ]]; then
        path="${path%/mod}"
    fi
    basename "$path"
}

source_module_path_for() {
    local path="${1#cas-cli/src/}"
    path="${path%.rs}"
    if [[ "$path" == */mod ]]; then
        path="${path%/mod}"
    fi
    printf '%s\n' "${path//\//::}"
}

source_public_symbols_for() {
    local source_file="$1"
    # Keep the extractor deliberately Rust-shaped and conservative. `pub`,
    # `pub(crate)`, and `pub(super)` declarations are the public surface an
    # integration test can exercise; private unit helpers are not consumers.
    sed -nE \
        -e 's/^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+(async[[:space:]]+)?fn[[:space:]]+([[:alnum:]_]+).*/\3/p' \
        -e 's/^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+(struct|enum|const|type)[[:space:]]+([[:alnum:]_]+).*/\3/p' \
        "$source_file" | sort -u
}

discover_source_integration_targets() {
    local source_path="$1" source_file module_path test_path symbol symbol_filter
    local -a symbol_patterns=()
    source_file="$repo_root/$source_path"
    [[ -f "$source_file" ]] || return 0
    module_path="$(source_module_path_for "$source_path")"

    while IFS= read -r symbol; do
        [[ -n "$symbol" ]] || continue
        symbol_filter="$symbol"
        [[ "$symbol_filter" == factory_* ]] && symbol_filter="${symbol_filter#factory_}"
        symbol_patterns+=(
            -e
            "^[[:space:]]*(pub([[:space:]]*\\([^)]*\\))?[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[[:alnum:]_]*${symbol_filter}[[:alnum:]_]*[[:space:]]*\\("
        )
    done < <(source_public_symbols_for "$source_file")

    # Integration tests normally consume a service through its public parent
    # API rather than importing the private source module. Explicit `use` /
    # path references remain useful evidence for modules that are public in a
    # crate, and source-path literals are accepted only as an anchored path
    # reference (not as arbitrary target-name text).
    while IFS= read -r test_path; do
        [[ -n "$test_path" ]] || continue
        if rg -q -e \
            "^[[:space:]]*(pub[[:space:]]+)?use[[:space:]].*${module_path}([[:space:];:{]|$)" \
            "$test_path" \
            || rg -q -e \
            "^[[:space:]]*[^/].*${module_path}" \
            "$test_path" \
            || rg -q -e \
            "^[[:space:]]*[^/].*(include_str!|include_bytes!|Path|read_to_string).*${source_path}" \
            "$test_path"; then
            add_required_test_target "$(test_target_for_path "$test_path")"
        fi

        # Service methods commonly have a `factory_` implementation prefix
        # while the public integration test names use the API suffix
        # (`factory_worker_status` -> `test_worker_status_*`). Match only test
        # declarations, so comments and fixture strings cannot claim a target.
        if [[ ${#symbol_patterns[@]} -gt 0 ]] \
            && rg -q "${symbol_patterns[@]}" "$test_path"; then
            add_required_test_target "$(test_target_for_path "$test_path")"
        fi
    done < <(rg --files --glob '*.rs' cas-cli/tests 2>/dev/null || true)
}

builtin_catalog_path_for() {
    local relative="$1" skill
    case "$relative" in
        skills/*/*|agents/*.md)
            printf '%s\n' "$relative"
            ;;
        skills/*.md)
            skill="${relative#skills/}"
            skill="${skill%.md}"
            printf 'skills/%s/SKILL.md\n' "$skill"
            ;;
    esac
}

builtin_relative_path_for() {
    local path="${1#cas-cli/src/builtins/}"
    case "$path" in
        codex/*|grok/*)
            path="${path#*/}"
            ;;
    esac
    printf '%s\n' "$path"
}

discover_builtin_test_targets() {
    local literal="$1" test_paths test_path target rg_status
    if test_paths="$(rg -l -F --glob '*.rs' -- "$literal" cas-cli/tests)"; then
        :
    else
        rg_status=$?
        if [[ "$rg_status" -eq 1 ]]; then
            test_paths=''
        else
            printf 'SCOPED PROOF SURFACE: builtin path discovery failed for %s (rg exit %s).\n' \
                "$literal" "$rg_status" >&2
            exit 2
        fi
    fi
    while IFS= read -r test_path; do
        [[ -n "$test_path" ]] || continue
        target="$(test_target_for_path "$test_path")"
        [[ -n "$target" ]] || continue
        add_required_test_target "$target"
    done <<<"$test_paths"
}

lib_filter_covers() {
    local module="$1" filter
    # An unfiltered --lib run covers every library module. A module filter must
    # name the module itself, not one newly-added test inside it.
    [[ ${#filters[@]} -eq 0 ]] && return 0
    for filter in "${filters[@]}"; do
        [[ "$filter" == "$module" || "$filter" == *"::$module" ]] && return 0
    done
    return 1
}

required_lib_modules=()
required_test_targets=()
while IFS= read -r path; do
    case "$path" in
        cas-cli/src/*.rs|cas-cli/src/*/*.rs|cas-cli/src/*/*/*.rs|cas-cli/src/*/*/*/*.rs|cas-cli/src/*/*/*/*/*.rs)
            source_file="$repo_root/$path"
            [[ -f "$source_file" ]] || continue
            found_test_module=false
            while IFS= read -r module; do
                [[ -z "$module" ]] && continue
                required_lib_modules+=("$module")
                found_test_module=true
            done < <(sed -nE 's/^[[:space:]]*mod[[:space:]]+([[:alnum:]_]*tests)[[:space:]]*\{.*/\1/p' "$source_file")
            if ! "$found_test_module"; then
                required_lib_modules+=("$(source_module_name_for "$path")")
            fi
            discover_source_integration_targets "$path"
            ;;
        cas-cli/tests/*/*.rs)
            nested="${path#cas-cli/tests/}"
            add_required_test_target "$(integration_target_for "$nested")"
            ;;
        cas-cli/tests/*.rs)
            add_required_test_target "$(basename "${path%.rs}")"
            ;;
    esac
done < <(git diff --name-only "$merge_base" HEAD)

# These guardrails are file-class contracts, not optional path discoveries:
# an integration-test source must remain readable in nextest archives, and a
# hook source must preserve the handler-level JSON schema contract.
while IFS= read -r path; do
    case "$path" in
        cas-cli/tests/*)
            add_required_test_target builtin_archive_portability_test
            ;;
        cas-cli/src/hooks/*|cas-cli/src/cli/hook/*|.codex/hooks.json)
            add_required_test_target hook_schema
            ;;
    esac
done < <(git diff --name-only "$merge_base" HEAD)

# Builtin skills and agents are embedded into all harness flavors, so their
# source paths can be covered by tests that are not themselves changed. Always
# require the cross-flavor and agent contracts, then discover any additional
# guardrail binaries that name the changed builtin path literally. This keeps
# size/phrase tests coupled to the files they read without maintaining a
# hand-written path-to-test table. Search both source-shaped and installed
# catalog-shaped spellings because tests use both forms.
while IFS= read -r path; do
    is_builtin_skill_or_agent_path "$path" || continue
    add_required_test_target builtin_flavor_drift_test
    add_required_test_target agent_definition_contract_test
    add_required_test_target factory_codex_skill_guardrails
    # Every builtin_* binary is a guardrail for one of the embedded catalogs;
    # requiring the complete family keeps a newly-added size/phrase contract
    # from becoming invisible to --proof.
    case "$path" in
        */skills/*|*.md)
            for candidate in cas-cli/tests/builtin_*.rs; do
                [[ -f "$candidate" ]] || continue
                add_required_test_target "$(basename "${candidate%.rs}")"
            done
            ;;
    esac
    relative="$(builtin_relative_path_for "$path")"
    catalog_path="$(builtin_catalog_path_for "$relative")"
    discover_builtin_test_targets "$path"
    [[ -n "$catalog_path" ]] || continue
    discover_builtin_test_targets "$catalog_path"
done < <(git diff --name-only "$merge_base" HEAD)

missing=()
for module in "${required_lib_modules[@]}"; do
    if ! "$lib_requested" || ! lib_filter_covers "$module"; then
        missing+=("library module '$module'")
    fi
done
for target in "${required_test_targets[@]}"; do
    if ! contains_exact "$target" "${test_targets[@]}"; then
        missing+=("integration target '$target'")
    fi
done

if [[ ${#missing[@]} -eq 0 ]]; then
    echo "SCOPED PROOF SURFACE: covered committed diff from ${base_ref} ($(git rev-parse --short "$merge_base"))."
    proof_targets=()
    for module in "${required_lib_modules[@]}"; do
        proof_targets+=("lib:${module}")
    done
    for target in "${required_test_targets[@]}"; do
        proof_targets+=("test:${target}")
    done
    if [[ ${#proof_targets[@]} -eq 0 ]]; then
        proof_targets+=(none)
    fi
    (IFS=,; echo "SCOPED_PROOF: targets=${proof_targets[*]} result=PASS")
    exit 0
fi

echo "SCOPED PROOF INCOMPLETE: diff ${base_ref}@$(git rev-parse --short "$merge_base") is not covered." >&2
for item in "${missing[@]}"; do
    echo "  - missing ${item}" >&2
done
printf 'Run scripts/run-scoped-tests.sh --proof' >&2
printf ' %q' "${requested_args[@]}" >&2
printf '\n' >&2
exit 1
