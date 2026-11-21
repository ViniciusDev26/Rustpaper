#!/usr/bin/env bash
# run_grain.sh [experiment...]: captures each make_grain.py scene in WE,
# renders it with Rustpaper (30 fps, like batch.sh) and prints the grain
# statistics of both (measure_grain.py).
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(g_default g_s2 g_a0.5)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/$e"
    python3 "$HARNESS_DIR/calibration/make_grain.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>"$o/rp_warnings.txt"
    echo "== $e"
    python3 "$HARNESS_DIR/calibration/measure_grain.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
