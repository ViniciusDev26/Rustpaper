#!/usr/bin/env bash
# run_particles.sh [experiment...]: captures each make_particles.py scene in WE,
# renders it with Rustpaper from the same scene time, prints both statistics.
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(tvr tvr_off3 tvr_offm05 tvr_scale01 turb turb_mask_x vel)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/p_$e"
    python3 "$HARNESS_DIR/calibration/make_particles.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    warm=6; [ "$e" = rare ] && warm=0 # rare: watch the very first seconds
    # captures taken at another WE frame-rate limit are stale (WE sub-steps
    # and weakens operators at low fps): recapture them
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || WE_WARMUP=$warm WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>/dev/null
    echo "== $e"
    python3 "$HARNESS_DIR/calibration/measure_particles.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
