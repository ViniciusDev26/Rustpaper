#!/usr/bin/env bash
# run_parallax.sh [delay...]: camera parallax calibration.
#
# For each cameraparallaxdelay, builds the make_scene.py coordinate scene with
# camera parallax on (amount 0.2, mouse influence 1, the image at
# parallaxDepth 1), captures it in WE from the moment the window opens
# (no warm-up) and prints, over time, which scene point WE shows at the
# screen center (measure_parallax.py) next to Rustpaper rendering the same
# scene with the cursor WE saw (we_pointer.txt). The curve shows how WE
# follows the cursor (delay model) and where it ends (pointer and scale).
source "$(dirname "$0")/../lib.sh"
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
delays=("$@"); [ ${#delays[@]} -gt 0 ] || delays=(0.1 1.0 1.1 3.0)
for dl in "${delays[@]}"; do
    d="$REPO_DIR/harness-out/calib/parallax_$dl"; o="$d/out"
    python3 "$HARNESS_DIR/calibration/make_scene.py" "$d" 1920 1080 >/dev/null
    python3 - "$d" "$dl" <<'EOF'
import json, sys
d, delay = sys.argv[1], float(sys.argv[2])
p = f"{d}/scene.json"
s = json.load(open(p))
s["general"].update({"cameraparallax": True, "cameraparallaxamount": 0.2,
                     "cameraparallaxmouseinfluence": 1.0, "cameraparallaxdelay": delay})
s["objects"][0]["parallaxDepth"] = "1 1"
json.dump(s, open(p, "w"), indent=1)
pj = f"{d}/project.json"
j = json.load(open(pj)); j["title"] = f"parallax calib delay {delay}"
j["general"] = {"properties": {}}
json.dump(j, open(pj, "w"), indent=1)
EOF
    mkdir -p "$o"
    if [ ! -s "$o/we.mp4" ] || [ "${RECAPTURE:-0}" = 1 ]; then
        WE_WARMUP=0 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 10 >/dev/null || { echo "$dl: capture failed"; continue; }
    fi
    "$bin" "$d" --video "$o/rp.mp4" --duration 10 --fps 30 --screen 1920x1080 \
        --start "$(capture_time "$o")" --pointer "$(capture_pointer "$o")" 2>/dev/null
    echo "== delay $dl  (WE saw pointer $(capture_pointer "$o"), recording began at scene t=$(capture_time "$o"))"
    python3 "$HARNESS_DIR/calibration/measure_parallax.py" "$o/we.mp4" "$o/rp.mp4"
done
