#!/usr/bin/env bash
# isolate_effects.sh <workshop id> <object id> [effect index...]
#
# Effect ablation: for a layer with many effects, captures in WE and renders
# with Rustpaper one copy of the scene per effect, where that layer keeps
# ONLY that effect (the others are hidden; other objects are untouched), plus
# `none` (all its effects hidden). A variant can also be a set: `1+2`. compare.sh's metrics per variant show
# which effect makes Rustpaper's motion energy or mean SSIM differ.
source "$(dirname "$0")/../lib.sh"
id=$1 obj=$2; shift 2
src=$(wallpaper_dir "$id") || { echo "no wallpaper $id"; exit 1; }
base="$REPO_DIR/harness-out/calib/fx_${id}_${obj}"
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
mkdir -p "$base/src"
if [ ! -s "$base/src/scene.json" ]; then
    if [ -f "$src/scene.pkg" ]; then
        python3 "$HARNESS_DIR/calibration/unpack_pkg.py" "$src/scene.pkg" "$base/src"
    fi
    cp -r "$src"/. "$base/src"/
    rm -f "$base/src/scene.pkg"
fi
n=$(python3 -c "
import json, sys
s = json.load(open(sys.argv[1], encoding='utf-8-sig'))
o = next(o for o in s['objects'] if o.get('id') == int(sys.argv[2]))
print(len(o.get('effects', [])))" "$base/src/scene.json" "$obj")
variants=("$@"); [ ${#variants[@]} -gt 0 ] || variants=(none $(seq 0 $((n - 1))))
printf '%-6s %-45s %9s %9s %s\n' variant effect "mean SSIM" "energy" "(rp/we)"
for v in "${variants[@]}"; do
    d="$base/$v"; o="$d/out"
    if [ ! -s "$d/scene.json" ]; then
        mkdir -p "$d"; cp -r "$base/src"/. "$d"/
        python3 - "$d/scene.json" "$obj" "$v" <<'PY'
import json, sys
path, obj, v = sys.argv[1], int(sys.argv[2]), sys.argv[3]
s = json.load(open(path, encoding="utf-8-sig"))
o = next(o for o in s["objects"] if o.get("id") == obj)
for k, e in enumerate(o.get("effects", [])):
    # a variant is an index or a "+"-joined set (e.g. 1+2: two effects)
    e["visible"] = v != "none" and k in {int(x) for x in v.split("+")}
json.dump(s, open(path, "w"), indent=1)
PY
    fi
    mkdir -p "$o"
    [ "$(cat "$o/we_fps.txt" 2>/dev/null)" = "$WE_FPS" ] || rm -f "$o/we.mp4"
    [ -s "$o/we.mp4" ] || WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" 8 >/dev/null || { echo "$v: capture failed"; continue; }
    "$bin" "$d" --video "$o/rp.mp4" --duration 8 --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" \
        --pointer "$(capture_pointer "$o")" 2>"$o/rp_warnings.txt" || { echo "$v: render failed"; continue; }
    "$HARNESS_DIR/compare.sh" "$o/rp.mp4" "$o/we.mp4" "$o" >/dev/null
    name=$([ "$v" = none ] && echo "(no effect)" || python3 -c "
import json, sys
s = json.load(open(sys.argv[1]))
o = next(o for o in s['objects'] if o.get('id') == int(sys.argv[2]))
print('+'.join(o['effects'][int(x)]['file'].split('/')[-2][:12] for x in sys.argv[3].split('+')))" "$d/scene.json" "$obj" "$v")
    printf '%-6s %-45s %9s %9s %s\n' "$v" "$name" "$(cut -f1 "$o/metrics.tsv")" "$(cut -f5 "$o/metrics.tsv")" \
        "($(cut -f3 "$o/metrics.tsv")/$(cut -f4 "$o/metrics.tsv"))"
done
