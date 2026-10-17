#!/usr/bin/env bash
# Supervisor-executed cell matrix for cas-70a8 independent QA round 2 (reviewer silent-stork-66).
# Run as the supervisor, unchanged environment. Candidate: a private copy of the root-built 4cedda1 binary.
# Usage: run-root.sh <candidate-cas> [<base-cas>]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd -P)"; C="$HERE/../cells"
mkdir -p /mnt/rewind/cas/scratch/qa70a8r2/bin
cp "$1" /mnt/rewind/cas/scratch/qa70a8r2/bin/cand; CAND=/mnt/rewind/cas/scratch/qa70a8r2/bin/cand
"$CAND" --version > "$C/candidate-version.txt"
P="python3 -I $HERE/accept-proof.py"
ASSET=garbage $P "$CAND" "$C/cand-y-garbage-80"  y 80
ASSET=garbage $P "$CAND" "$C/cand-y-garbage-120" y 120
ASSET=slow    $P "$CAND" "$C/cand-y-slow-80"     y 80
ASSET=500     $P "$CAND" "$C/cand-y-500-80"      y 80
ASSET=garbage $P "$CAND" "$C/cand-n-80"          n 80
ASSET=garbage $P "$CAND" "$C/cand-yes-garbage-80" yes 80
ASSET=garbage $P "$CAND" "$C/cand-pipe-garbage"  pipe
ASSET=garbage LC=C     $P "$CAND" "$C/cand-y-garbage-C"       y 80
ASSET=garbage NOCOLOR=1 $P "$CAND" "$C/cand-y-garbage-nocolor" y 80
if [ $# -ge 2 ]; then
  cp "$2" /mnt/rewind/cas/scratch/qa70a8r2/bin/base; BASE=/mnt/rewind/cas/scratch/qa70a8r2/bin/base
  "$BASE" --version > "$C/base-version.txt"
  ASSET=slow ALLOW_BASE=1 $P "$BASE" "$C/base-y-slow-80" y 80
fi
echo DONE
