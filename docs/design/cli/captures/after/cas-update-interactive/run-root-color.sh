#!/usr/bin/env bash
# Supervisor-executed colour controls for cas-70a8 QA round 2: same driver, inherited NO_COLOR dropped for the child only.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd -P)"; C="$HERE/../cells"; CAND=/mnt/rewind/cas/scratch/qa70a8r2/bin/cand
P="python3 -I $HERE/accept-proof.py"
COLOR=1 ASSET=garbage $P "$CAND" "$C/cand-y-garbage-80-color" y 80
COLOR=1 ASSET=slow    $P "$CAND" "$C/cand-y-slow-120-color"   y 120
COLOR=1 ASSET=garbage $P "$CAND" "$C/cand-n-80-color"         n 80
echo DONE
