#!/usr/bin/env bash
# run_shake.sh [experiment...]: calibrates `general.camerashake` alone.
# Each experiment is a make_scene.py coordinate scene (pixels encode their
# scene position) with camera shake on; measure_shake.py tracks the camera
# offset frame by frame in the WE capture and in the Rustpaper render.
# Experiment names: s<amplitude>_v<speed>_r<roughness>, e.g. s0.5_v1_r0.
source "$(dirname "$0")/../lib.sh"
exps=("$@"); [ ${#exps[@]} -gt 0 ] || exps=(s0.5_v1_r0 s0.5_v1_r1 s0.5_v0.64_r1)
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
for e in "${exps[@]}"; do
    d="$REPO_DIR/harness-out/calib/shake_$e"
    python3 "$HARNESS_DIR/calibration/make_scene.py" "$d" 1920 1080 >/dev/null
    python3 - "$d/scene.json" "$e" <<'PY'
import json, sys
path, e = sys.argv[1], sys.argv[2]
amp, speed, rough = (float(p[1:]) for p in e.split("_"))
s = json.load(open(path))
s["general"].update({"camerashake": True, "camerashakeamplitude": amp,
                     "camerashakespeed": speed, "camerashakeroughness": rough})
json.dump(s, open(path, "w"), indent=1)
PY
    o="$d/out"; mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$e: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>"$o/rp_warnings.txt"
    echo "== $e"
    python3 "$HARNESS_DIR/calibration/measure_shake.py" --fit "$(capture_time "$o")" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
done
