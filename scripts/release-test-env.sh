#!/usr/bin/env bash
# Shared suite-child environment boundary. Orchestrators retain their controls;
# only the subprocess loses identity, train/gate knobs and receipt destinations.
# CAS_HOST_MEMORY_LEASE is a separately validated resource capability: retaining
# it prevents nested admitted suites from waiting on their owning proof.
release_test_env_scrub() {
    local release_env_key
    if [[ -n "${release_test_home:-}" ]]; then
        # Preserve toolchain locations while tests see a clean CI-like HOME.
        export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
        export HOME="$release_test_home"
    fi
    export GIT_CONFIG_GLOBAL=/dev/null
    for release_env_key in CAS_FACTORY_SESSION CAS_AGENT_ROLE CAS_AGENT_NAME \
        CAS_SUPERVISOR_NAME CAS_AGENT_ID CAS_SESSION_ID CAS_ROOT CAS_CLONE_PATH \
        CAS_FACTORY_MODE CAS_FACTORY_SUPERVISOR_CLI CAS_FACTORY_WORKER_CLI \
        AI_AGENT CLAUDECODE CLAUDE_CODE_CHILD_SESSION \
        CAS_RELEASE_ARTIFACTS_ROOT CAS_RELEASE_RECEIPTS_RUN_DIR CAS_RELEASE_ENV_FILE \
        CAS_RELEASE_EPIC_REF VERIFIED_TEST_COUNT_FILE VERIFIED_TEST_LOG \
        MAKEFLAGS MFLAGS GNUMAKEFLAGS MAKELEVEL; do
        unset "$release_env_key"
    done
    while IFS= read -r release_env_key; do
        case "$release_env_key" in
            CAS_RELEASE_TRAIN_*|CAS_RELEASE_GATE_*|RELEASE_GATE_*) unset "$release_env_key" ;;
        esac
    done < <(compgen -e)
}

release_test_child() (
    release_test_env_scrub
    "$@"
)

# The memory guard executes argv, so compile children use this same file as
# an executable adapter rather than trying to export a shell function.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    if [[ "${1:-}" == --home ]]; then
        release_test_home="$2"
        shift 2
    fi
    release_test_env_scrub
    exec "$@"
fi
