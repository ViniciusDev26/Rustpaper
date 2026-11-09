#!/usr/bin/env bash
# pending.sh: the captures that could not run while the session was locked.
#   1. recapture scenes whose WE capture was contaminated (lock screen, WE
#      crash dialog): harness-out/recapture_after_unlock.txt
#   2. second particle calibration round (trails, sizes, emitters, alpha
#      oscillation, perspective)
#   3. re-render + re-compare everything with the current renderer
# Run it with the session unlocked; it refuses otherwise.
source "$(dirname "$0")/lib.sh"
session_locked && die "unlock the session first (captures would record the lock screen)"
redo="$REPO_DIR/harness-out/recapture_after_unlock.txt"
if [ -s "$redo" ]; then
    "$HARNESS_DIR/batch.sh" --recapture --update $(cat "$redo") && rm -f "$redo"
fi
"$HARNESS_DIR/calibration/run_particles.sh" trail trail_long size box sphere_dir oscalpha persp
BATCH_JOBS=${BATCH_JOBS:-6} "$HARNESS_DIR/batch.sh"
python3 "$HARNESS_DIR/score.py"
