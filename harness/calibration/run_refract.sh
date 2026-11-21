#!/usr/bin/env bash
# run_refract.sh: refraction strength of still drops (make_refract.py), WE vs
# Rustpaper. Prints mean |frame with drops - frame without| (0-255, RGB) for
# a 4K scene and a 1080p one. RENDER_ENV is passed to the Rustpaper renders
# (e.g. an experiment flag).
source "$(dirname "$0")/../lib.sh"
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
frame() { ffmpeg -v error -ss 2 -i "$1" -frames:v 1 -vf "crop=1920:1000:0:40" -f rawvideo -pix_fmt rgb24 -; }
for base in ${REFR_SETS:-refr_1080 refr_4k}; do
    for e in "$base" "${base}_bg"; do
        d="$REPO_DIR/harness-out/calib/$e"
        python3 "$HARNESS_DIR/calibration/make_refract.py" "$d" "$e" >/dev/null
        o="$d/out"; mkdir -p "$o"
        [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
        [ -s "$o/we.mp4" ] || "$HARNESS_DIR/we-capture.sh" "$d" "$o" 4 >/dev/null || { echo "$e: capture failed"; continue 2; }
        env ${RENDER_ENV:-} "$bin" "$d" --video "$o/rp.mp4" --duration 4 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>/dev/null
    done
    for side in we rp; do
        a="$REPO_DIR/harness-out/calib/$base/out/$side.mp4"; b="$REPO_DIR/harness-out/calib/${base}_bg/out/$side.mp4"
        python3 - "$side" <(frame "$a") <(frame "$b") <<'PY'
import sys
s, a, b = sys.argv[1], open(sys.argv[2], "rb").read(), open(sys.argv[3], "rb").read()
d = [abs(x - y) for x, y in zip(a, b)]
print(f"  {s}: mean |drops - bg| {sum(d) / len(d):.3f}   pixels changed >8: {sum(x > 8 for x in d[::3]) / (len(d) // 3):.1%}")
PY
    done | sed "s/^/$base/"
done
