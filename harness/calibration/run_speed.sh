#!/usr/bin/env bash
# run_speed.sh [experiment...]: remapvalue -> speed calibration (make_particles.py spd_*).
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(spd_none spd_c3 spd_c3_f3)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/p_$e"
    python3 "$HARNESS_DIR/calibration/make_particles.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>/dev/null
    echo "== $e"
    python3 "$HARNESS_DIR/calibration/measure_speed.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
