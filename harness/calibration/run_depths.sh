#!/usr/bin/env bash
# run_depths.sh [name=reze] [W H amount influence delay]: camera parallax by depth.
#
# Builds the make_depths.py scene (five squares at parallaxDepth -0.5,
# -0.11, 0.12, 0.5 and 1.0; defaults are Chainsaw Man 3577990983's camera
# parallax in its 3840x2160 scene), captures it in WE (warm-up long enough
# for the parallax follow to settle: delay 1.92 needs ~5 time constants),
# renders it with Rustpaper at the cursor WE saw (we_pointer.txt) and prints
# where each square sits, as an offset from its no-parallax position, in both.
source "$(dirname "$0")/../lib.sh"
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
name=${1:-reze}; shift || true
d="$REPO_DIR/harness-out/calib/depths_$name"; o="$d/out"
python3 "$HARNESS_DIR/calibration/make_depths.py" "$d" "$@" >/dev/null
mkdir -p "$o"
if [ ! -s "$o/we.mp4" ] || [ "${RECAPTURE:-0}" = 1 ]; then
    WE_WARMUP=${WE_WARMUP:-12} "$HARNESS_DIR/we-capture.sh" "$d" "$o" 4 >/dev/null || die "capture failed"
fi
"$bin" "$d" --video "$o/rp.mp4" --duration 4 --fps 30 --screen 1920x1080 \
    --start "$(capture_time "$o")" --pointer "$(capture_pointer "$o")" 2>/dev/null
echo "== $name (WE saw pointer $(capture_pointer "$o"), recording began at scene t=$(capture_time "$o"))"
python3 "$HARNESS_DIR/calibration/measure_depths.py" "$o/we.mp4" "$o/rp.mp4"
