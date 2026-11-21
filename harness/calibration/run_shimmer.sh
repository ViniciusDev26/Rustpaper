#!/usr/bin/env bash
# run_shimmer.sh [experiment...]: the shimmer effect alone, WE vs Rustpaper
# (make_shimmer.py + measure_sweep.py).
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(sh_v1_d2 sh_v0.4_d3 sh_v0.4_d3_b31)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/$e"
    python3 "$HARNESS_DIR/calibration/make_shimmer.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>"$o/rp_warnings.txt"
    echo "== $e (WE scene time at start $(capture_time "$o") s)"
    python3 "$HARNESS_DIR/calibration/measure_sweep.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
