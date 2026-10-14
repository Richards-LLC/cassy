#!/usr/bin/env bash

# The proof helper; fixtures substitute a recording stand-in.
assembly_proof_run() {
    if [[ -n "${CAS_RELEASE_TRAIN_ASSEMBLY_PROOF:-}" ]]; then
        "$CAS_RELEASE_TRAIN_ASSEMBLY_PROOF" "$@"
    else
        python3 "$script_dir/assembly-proof.py" "$@"
    fi
}

# Record which proof stands for the assembled code input, so the gate can
# reuse exactly that proof's rows (release-gate.sh CAS_RELEASE_GATE_INTEGRATION_PROOF).
record_integration_proof() {
    local source="$1" line="$2" receipt="$run_dir/assemble.integration.json"
    python3 - "$receipt" "$source" "$line" <<'PY_INTEGRATION_PROOF'
import json, re, sys
path, source, line = sys.argv[1:]
value = json.load(open(path))
fields = dict(re.findall(r"(\w+)=(\S+)", line))
value["integration_proof"] = {"source": source, **{key: fields[key] for key in
                              ("receipt", "source_sha", "code_input", "run") if key in fields}}
with open(path + ".tmp", "w") as stream:
    stream.write(json.dumps(value) + "\n")
import os
os.replace(path + ".tmp", path)
PY_INTEGRATION_PROOF
}

# The integration proof recorded for this run's from-main assembly, if any.
integration_proof_source() {
    local receipt="$run_dir/assemble.integration.json"
    [[ -r "$receipt" ]] || return 1
    python3 - "$receipt" <<'PY_INTEGRATION_SOURCE'
import json, sys
try:
    proof = json.load(open(sys.argv[1])).get("integration_proof") or {}
except (OSError, ValueError, AttributeError):
    sys.exit(1)
source = proof.get("source") if isinstance(proof, dict) else None
if source not in ("merge-queue", "assembly"):
    sys.exit(1)
print(source)
PY_INTEGRATION_SOURCE
}

# cas-2970: the merge queue is the integration proof. A release assembled from
# main reuses the successful merge-queue run that proved main's code input; the
# local two-context assembly proof runs only when no such run matches (a code
# change, an expired or missing receipt, or GitHub unavailable). A sweep-backed
# assembly keeps its assembly proof, which normally reuses the sweep's receipt.
assemble_integration_proof() {
    local line
    [[ -f "$worktree/Cargo.toml" ]] || return 0
    if declare -F from_main_full_gate_required >/dev/null && from_main_full_gate_required; then
        if line="$(assembly_proof_run reuse-merge-queue "$worktree")"; then
            printf '%s\n' "$line"
            printf 'integration proof: the merge queue proved this code input; no local suite run\n'
            record_integration_proof merge-queue "$line"
            return 0
        fi
        printf 'integration proof: no merge-queue receipt matches this code input; running the assembly proof fresh\n'
        # Stream the long proof's progress; keep its output for the receipt.
        assembly_proof_run prove "$worktree" | tee "$run_dir/assembly-proof.out"
        local rc="${PIPESTATUS[0]}"
        ((rc == 0)) || return "$rc"
        line="$(grep '^PASS assembly ' "$run_dir/assembly-proof.out" | tail -1)"
        record_integration_proof assembly "$line"
        return 0
    fi
    assembly_proof_run prove "$worktree"
}

cut_stage_assemble() {
    if cut_has_external_stage assemble; then
        cut_run_external_stage assemble
    else
        python3 "$script_dir/release-integrate.py" "$worktree" || return $?
        assemble_integration_proof
    fi
}
