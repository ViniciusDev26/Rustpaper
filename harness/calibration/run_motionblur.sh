#!/usr/bin/env bash
# run_motionblur.sh [experiment...]: captures each make_motionblur.py scene in
# WE, renders it with Rustpaper (30 fps, like batch.sh) and prints the trail
# of both (measure_trail.py).
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(mb_none mb_0.8 mb_0.3 mb_0.13)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/$e"
    python3 "$HARNESS_DIR/calibration/make_motionblur.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>"$o/rp_warnings.txt"
    echo "== $e (WE at $WE_FPS fps)"
    rate=(); [[ $e == mbr_* ]] && rate=(--rate "${e#mbr_}")
    python3 "$HARNESS_DIR/calibration/measure_trail.py" "${rate[@]}" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
