#!/usr/bin/env bash
# isolate.sh <workshop id> <object id> [object id...]: captures in WE and
# renders with Rustpaper a copy of a scene that keeps ONLY the given objects
# (on a black background), then prints measure_particles.py statistics.
# Finds which object of a failing scene differs from WE.
source "$(dirname "$0")/../lib.sh"
id=$1; shift
objs=$(IFS=,; echo "$*")
src=$(wallpaper_dir "$id") || { echo "no wallpaper $id"; exit 1; }
d="$REPO_DIR/harness-out/calib/iso_${id}_${objs//,/_}"
o="$d/out"; mkdir -p "$o"
if [ ! -s "$d/scene.json" ]; then
    if [ -f "$src/scene.pkg" ]; then
        python3 "$HARNESS_DIR/calibration/unpack_pkg.py" "$src/scene.pkg" "$d"
        cp "$src/project.json" "$d/"
    else
        cp -r "$src"/. "$d"/
    fi
    python3 - "$d/scene.json" "$objs" <<'PY'
import json, sys
path, keep = sys.argv[1], {int(x) for x in sys.argv[2].split(",")}
s = json.load(open(path, encoding="utf-8-sig"))
s["objects"] = [o for o in s["objects"] if o.get("id") in keep]
for o in s["objects"]:
    o.pop("parent", None)
g = s.setdefault("general", {})
g.update({"clearcolor": "0 0 0", "clearenabled": True, "bloom": False, "cameraparallax": False})
json.dump(s, open(path, "w"), indent=1)
PY
fi
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
[ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
[ -s "$o/we.mp4" ] || WE_WARMUP=6 WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "capture failed"; exit 1; }
"$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" 2>/dev/null
python3 "$HARNESS_DIR/calibration/measure_particles.py" "$o/we.mp4" "$o/rp.mp4" | sed "s|$o/||"
