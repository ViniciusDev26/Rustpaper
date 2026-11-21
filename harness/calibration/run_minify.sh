#!/usr/bin/env bash
# run_minify.sh [experiment...]: how a 4K layer is minified onto a 1080p
# screen (make_minify.py), WE vs Rustpaper, read with measure_grain.py
# (level and std of a crop: ~0 std = filtered, high std = aliasing).
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(m_1mip m_mips m_1mip_fx m_mips_fx)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/$e"
    [ -s "$d/scene.json" ] || python3 "$HARNESS_DIR/calibration/make_minify.py" "$d" "$e" >/dev/null
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || "$HARNESS_DIR/we-capture.sh" "$d" "$o" 4 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 4 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>"$o/rp_warnings.txt"
    echo "== $e"
    python3 "$HARNESS_DIR/calibration/measure_grain.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||" | cut -d'|' -f1
done
