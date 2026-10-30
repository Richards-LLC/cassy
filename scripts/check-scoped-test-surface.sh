#!/usr/bin/env bash
#
# Check whether one scoped-test invocation covers the committed source and
# integration-test surfaces it is being offered as proof for. This intentionally
# does not run cargo: run-scoped-tests.sh owns execution and calls this only
# after Cargo has reported a real, nonzero test count.

set -euo pipefail

# `SCOPED_PROOF_TARGET_BRANCH` carries the declared WorkTarget branch when the
# caller has not already resolved it to SCOPED_PROOF_BASE. The checker resolves
# the task's own baseline instead of assuming that every worker diff starts at
# origin/main.
target_branch="${SCOPED_PROOF_TARGET_BRANCH:-}"

usage() {
    echo "usage: $0 [--base <git-ref>] [--resolve-targets] [--paths-from-stdin] -- [cargo/nextest arguments]" >&2
    exit 2
}

base_ref=""
resolve_targets=false
paths_from_stdin=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || usage
            base_ref="$2"
            shift 2
            ;;
        --resolve-targets)
            resolve_targets=true
            shift
            ;;
        --paths-from-stdin)
            paths_from_stdin=true
            shift
            ;;
        --)
            shift
            break
            ;;
        *) usage ;;
    esac
done

[[ $# -gt 0 || "$resolve_targets" == true ]] || usage
requested_args=("$@")

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
    echo "SCOPED PROOF SURFACE: cannot inspect the committed diff outside a Git repository." >&2
    exit 2
}
cd "$repo_root"

# With auto-discovery disabled, a suite's source stem is no longer a Cargo
# binary name. Resolve aliases from the explicit harness module declarations.
consolidated_target_map=""
if [[ -f cas-cli/Cargo.toml ]] && grep -Eq '^autotests[[:space:]]*=[[:space:]]*false' cas-cli/Cargo.toml; then
    mapping_script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/cas-test-targets.py"
    consolidated_target_map="$(python3 "$mapping_script" "$repo_root/cas-cli")"
fi

resolve_test_target_name() {
    local wanted="$1" suite target
    if [[ -n "$consolidated_target_map" ]]; then
        while IFS='|' read -r suite target; do
            if [[ "$suite" == "$wanted" ]]; then
                printf '%s\n' "$target"
                return 0
            fi
        done <<<"$consolidated_target_map"
    fi
    printf '%s\n' "$wanted"
}

# Proof scope comes from committed Git paths. Keep the Git-only discovery
# contract (cas-81bc): a host rg must not stall close before its main scan.
grep_backend=git
tracked_rust_test_paths=('cas-cli/tests/*.rs' 'cas-cli/tests/**/*.rs')

run_test_path_search() {
    local mode="$1" pattern
    shift
    # One alternation lets Git compile a single regex instead of evaluating
    # hundreds of separate -e patterns. Stdin keeps large deliveries clear of
    # argv limits; fixed literals stay on separate lines for exact OR matching.
    {
        if [[ "$mode" == regex ]]; then
            printf -v pattern '%s|' "$@"
            printf '%s\n' "${pattern%|}"
        else
            printf '%s\n' "$@"
        fi
    } |
    case "$mode" in
        regex)
            git grep --no-color -l -E -f - -- "${tracked_rust_test_paths[@]}"
            ;;
        fixed)
            git grep --no-color -l -F -f - -- "${tracked_rust_test_paths[@]}"
            ;;
        *)
            return 2
            ;;
    esac
}

search_test_paths() {
    local mode="$1" matches search_status
    shift
    if matches="$(run_test_path_search "$mode" "$@")"; then
        [[ -n "$matches" ]] || return 0
        printf '%s\n' "$matches" | sort -u
    else
        search_status=$?
        return "$search_status"
    fi
}

if [[ -z "$base_ref" ]]; then
    if [[ -n "$target_branch" ]]; then
        base_ref="$target_branch"
    elif git rev-parse --verify --quiet origin/main >/dev/null; then
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
if [[ "$resolve_targets" == true ]]; then
    # Keep --resolve-targets stdout machine-readable for the runner and send
    # the human-facing baseline diagnostic to stderr.
    echo "SCOPED_PROOF SURFACE: base=${base_ref} merge-base=${merge_base}" >&2
else
    echo "SCOPED_PROOF SURFACE: base=${base_ref} merge-base=${merge_base}"
fi

changed_paths=()
if [[ "$paths_from_stdin" == true ]]; then
    while IFS= read -r path; do
        [[ -n "$path" ]] && changed_paths+=("$path")
    done
else
    mapfile -t changed_paths < <(git diff --name-only "$merge_base" HEAD)
fi

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
        --skip)
            # A skipped test is an exclusion, not a module filter: reading its
            # value as a filter made `-- --skip X` narrow the lib run to X and
            # fail the surface check (cas-e4d2). `--skip=X` is ignored below.
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

manifest_test_target_for_path() {
    local test_path wanted
    test_path="$1"
    wanted="${test_path#cas-cli/}"
    [[ -f cas-cli/Cargo.toml ]] || return 1
    awk -v wanted="$wanted" '
        function value(line) {
            sub(/^[^\"]*\"/, "", line)
            sub(/\".*$/, "", line)
            return line
        }
        /^\[/ {
            if (in_test && path == wanted && name != "") {
                print name
                found=1
                exit
            }
            in_test=($0 ~ /^\[\[test\]\][[:space:]]*$/)
            name=""
            path=""
            next
        }
        in_test && $0 ~ /^[[:space:]]*name[[:space:]]*=/ { name=value($0) }
        in_test && $0 ~ /^[[:space:]]*path[[:space:]]*=/ { path=value($0) }
        END {
            if (!found && in_test && path == wanted && name != "") print name
        }
    ' cas-cli/Cargo.toml
}

manifest_test_path_for_target() {
    local target="$1"
    [[ -f cas-cli/Cargo.toml ]] || return 1
    awk -v wanted="$target" '
        function value(line) {
            sub(/^[^\"]*\"/, "", line)
            sub(/\".*$/, "", line)
            return line
        }
        /^\[/ {
            if (in_test && name == wanted && path != "") {
                print path
                found=1
                exit
            }
            in_test=($0 ~ /^\[\[test\]\][[:space:]]*$/)
            name=""
            path=""
            next
        }
        in_test && $0 ~ /^[[:space:]]*name[[:space:]]*=/ { name=value($0) }
        in_test && $0 ~ /^[[:space:]]*path[[:space:]]*=/ { path=value($0) }
        END {
            if (!found && in_test && name == wanted && path != "") print path
        }
    ' cas-cli/Cargo.toml
}

cargo_test_target_exists() {
    local target="$1" manifest_path
    [[ -n "$target" ]] || return 1
    if [[ -z "$consolidated_target_map" ]]; then
        [[ -f "cas-cli/tests/${target}.rs" || -f "cas-cli/tests/${target}/main.rs" ]] && return 0
    fi
    manifest_path="$(manifest_test_path_for_target "$target" || true)"
    [[ -n "$manifest_path" && -f "cas-cli/${manifest_path}" ]]
}

integration_target_for() {
    local nested_path="$1" directory candidate manifest_target target
    directory="${nested_path%%/*}"

    manifest_target="$(manifest_test_target_for_path "cas-cli/tests/${nested_path}" || true)"
    if [[ -n "$manifest_target" ]] && cargo_test_target_exists "$manifest_target"; then
        printf '%s\n' "$manifest_target"
        return 0
    fi

    # GH #778: the nested path is often owned by the conventional top-level
    # integration target with the same stem (for example,
    # mcp_tools_test/task_tools/operations.rs belongs to mcp_tools_test.rs).
    # Resolve that filename before inspecting file contents so a fixture that
    # merely mentions the path cannot claim ownership.
    candidate="cas-cli/tests/${directory}.rs"
    target="$(resolve_test_target_name "$directory")"
    if [[ -f "${candidate}" ]] && cargo_test_target_exists "$target"; then
        printf '%s\n' "$target"
        return 0
    fi

    if [[ -f "cas-cli/tests/${directory}/main.rs" ]] && cargo_test_target_exists "$target"; then
        printf '%s\n' "$target"
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
            target="$(resolve_test_target_name "$(basename "${candidate%.rs}")")"
            if cargo_test_target_exists "$target"; then
                printf '%s\n' "$target"
                return 0
            fi
        fi
    done
    # A nested module without a Cargo root is not an integration target. Do
    # not emit its directory name: Cargo would reject it as a phantom target.
}

add_required_test_target() {
    local target="$1" known
    [[ -n "$target" ]] || return 0
    target="$(resolve_test_target_name "$target")"
    cargo_test_target_exists "$target" || return 0
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
        || "$path" == cas-cli/src/builtins/*/agents/* \
        || "$path" == cas-cli/src/builtins/jobs/* ]]
}

test_target_for_path() {
    local test_path="$1" manifest_target
    manifest_target="$(manifest_test_target_for_path "$test_path" || true)"
    if [[ -n "$manifest_target" ]] && cargo_test_target_exists "$manifest_target"; then
        printf '%s\n' "$manifest_target"
        return 0
    fi
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

collect_source_search_patterns() {
    # Integration tests normally consume a service through its public parent
    # API rather than importing the private source module. Explicit `use` /
    # path references remain useful evidence for modules that are public in a
    # crate, and source-path literals are accepted only as an anchored path
    # reference (not as arbitrary target-name text). Collect all changed source
    # paths before searching: close needs their union, not a per-path mapping.
    # Qualified-symbol patterns are already a subset of module::identifier;
    # repeating them only bloats the regex for modules with large public APIs.
    source_path_patterns+=("$1")
    source_module_patterns+=("$2")
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

discover_test_targets() {
    local mode="$1" test_paths test_path target search_status
    shift
    [[ $# -gt 0 ]] || return 0
    if test_paths="$(search_test_paths "$mode" "$@" 2>/dev/null)"; then
        :
    else
        search_status=$?
        if [[ "$search_status" -eq 1 ]]; then
            test_paths=''
        else
            printf 'SCOPED PROOF SURFACE: %s discovery failed (%s exit %s).\n' \
                "$mode" "$grep_backend" "$search_status" >&2
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
    # occur in the module path, so every test in that module matches it. A
    # filter naming one test below the module cannot cover its siblings.
    [[ ${#filters[@]} -eq 0 ]] && return 0
    for filter in "${filters[@]}"; do
        [[ "$module" == *"$filter"* || "$filter" == *"::$module" ]] && return 0
    done
    return 1
}

required_lib_modules=()
required_test_targets=()
source_path_patterns=()
source_module_patterns=()
while IFS= read -r path; do
    case "$path" in
        cas-cli/src/*.rs|cas-cli/src/*/*.rs|cas-cli/src/*/*/*.rs|cas-cli/src/*/*/*/*.rs|cas-cli/src/*/*/*/*/*.rs)
            source_file="$repo_root/$path"
            [[ -f "$source_file" ]] || continue
            module_path="$(source_module_path_for "$path")"
            found_test_module=false
            while IFS= read -r module; do
                [[ -z "$module" ]] && continue
                if [[ "$module_path" == lib ]]; then
                    required_lib_modules+=("$module")
                else
                    required_lib_modules+=("${module_path}::${module}")
                fi
                found_test_module=true
            done < <(sed -nE 's/^[[:space:]]*mod[[:space:]]+([[:alnum:]_]*tests)[[:space:]]*\{.*/\1/p' "$source_file")
            if ! "$found_test_module"; then
                required_lib_modules+=("$(source_module_name_for "$path")")
            fi
            collect_source_search_patterns "$path" "$module_path"
            ;;
        cas-cli/tests/*/*.rs)
            nested="${path#cas-cli/tests/}"
            add_required_test_target "$(integration_target_for "$nested")"
            ;;
        cas-cli/tests/*.rs)
            add_required_test_target "$(basename "${path%.rs}")"
            ;;
    esac
done < <(printf '%s\n' "${changed_paths[@]}")
if [[ ${#source_module_patterns[@]} -gt 0 ]]; then
    # Factor shared anchors around the path union instead of repeating the
    # complete expression per path. This reduces regex compiler overhead.
    printf -v module_alternation '%s|' "${source_module_patterns[@]}"
    printf -v path_alternation '%s|' "${source_path_patterns[@]}"
    module_alternation="(${module_alternation%|})"
    path_alternation="(${path_alternation%|})"
    discover_test_targets regex \
        "^[[:space:]]*(pub[[:space:]]+)?use[[:space:]].*${module_alternation}([[:space:];:{]|$)" \
        "^[[:space:]]*[^/].*([^[:alnum:]_]|^)${module_alternation}::[[:alnum:]_]" \
        "^[[:space:]]*[^/].*(include_str!|include_bytes!|Path|read_to_string).*${path_alternation}"
fi

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
done < <(printf '%s\n' "${changed_paths[@]}")

# Builtin skills and agents are embedded into all harness flavors, so their
# source paths can be covered by tests that are not themselves changed. Always
# require the cross-flavor and agent contracts, then discover any additional
# guardrail binaries that name the changed builtin path literally. This keeps
# size/phrase tests coupled to the files they read without maintaining a
# hand-written path-to-test table. Search both source-shaped and installed
# catalog-shaped spellings because tests use both forms.
builtin_search_literals=()
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
    builtin_search_literals+=("$path")
    [[ -n "$catalog_path" ]] || continue
    builtin_search_literals+=("$catalog_path")
done < <(printf '%s\n' "${changed_paths[@]}")
discover_test_targets fixed "${builtin_search_literals[@]}"

missing=()
if [[ "$resolve_targets" == true ]]; then
    emitted_lib_modules=()
    printf 'SCOPED_PROOF_TARGET_ARGS:'
    for module in "${required_lib_modules[@]}"; do
        if ! contains_exact "$module" "${emitted_lib_modules[@]}"; then
            emitted_lib_modules+=("$module")
        fi
    done
    # Cargo accepts one --lib flag followed by any number of positional
    # nextest filters. Repeating --lib makes the generated proof command
    # invalid before it can reach the surface guard.
    if [[ ${#emitted_lib_modules[@]} -gt 0 ]]; then
        printf ' --lib'
        printf ' %s' "${emitted_lib_modules[@]}"
    fi
    for target in "${required_test_targets[@]}"; do
        printf ' --test %s' "$target"
    done
    printf '\n'
    exit 0
fi

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
    # head= lets the close gate accept this receipt for the exact tip it
    # proves even when the gate derives a different base (cas-a9a1).
    (IFS=,; echo "SCOPED_PROOF: targets=${proof_targets[*]} result=PASS base=${merge_base} head=$(git rev-parse HEAD)")
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
