#!/usr/bin/env python3
"""make_shimmer.py <out dir> <experiment>

A flat grey 1920x1080 layer with the `shimmer` effect alone (no mask), to
calibrate its sweep against WE: measure_sweep.py tracks the mean level of a
strip over time. Experiments: sh_v<speed>_d<delay> (e.g. sh_v0.4_d3, the
values of Anime girl v40 3835806837), optional _b31 (BLENDMODE 31 like it).
"""
import json, os, shutil, struct, sys

out, exp = sys.argv[1], sys.argv[2]
parts = exp.split("_")
speed = float(parts[1][1:])
delay = float(parts[2][1:])
blend = int(parts[3][1:]) if len(parts) > 3 else None
W, H, TW, TH = 1920, 1080, 32, 18


def tex_rgba(w, h, px):
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1) + struct.pack("<I", 1)
    return d + struct.pack("<5I", w, h, 0, len(px), len(px)) + px


os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "grey.tex"), "wb").write(tex_rgba(TW, TH, bytes((96, 96, 96, 255)) * (TW * TH)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["grey"]}]},
          open(os.path.join(out, "materials", "grey.json"), "w"), indent=1)
json.dump({"material": "materials/grey.json"}, open(os.path.join(out, "models", "grey.json"), "w"))
ASSETS = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
    "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
src = os.path.join(ASSETS, "effects", "shimmer")
for sub in ("materials", "shaders"):
    shutil.copytree(os.path.join(src, sub), os.path.join(out, sub), dirs_exist_ok=True)
os.makedirs(os.path.join(out, "effects", "shimmer"), exist_ok=True)
shutil.copy(os.path.join(src, "effect.json"), os.path.join(out, "effects", "shimmer", "effect.json"))
p = {"id": 11, "constantshadervalues": {"ui_editor_properties_speed": speed, "ui_editor_properties_delay": delay}}
if blend is not None:
    p["combos"] = {"BLENDMODE": blend}
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "grey", "image": "models/grey.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0,
                 "effects": [{"file": "effects/shimmer/effect.json", "id": 10, "visible": True, "passes": [p]}]}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib shimmer {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
