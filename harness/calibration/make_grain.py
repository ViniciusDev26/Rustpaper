#!/usr/bin/env python3
"""make_grain.py <out dir> <experiment>

Builds a loose WE scene to calibrate the `filmgrain` effect alone: one
full-screen flat grey image layer with `filmgrain` on it (the effect's
materials and shaders copied in, as the editor packs them). measure_grain.py
compares the grain's spatial contrast and frame-to-frame change.

Experiments: g_default (the effect's defaults), g_s<scale> e.g. g_s2 (noise
scale), g_a<strength> e.g. g_a0.5; a `4k_` prefix (4k_g_default) builds a
3840x2160 scene (shown on the 1920x1080 screen); a `_tail` suffix adds a
neutral `opacity` effect (alpha 1) AFTER the grain, to see whether WE draws a
layer's LAST effect pass straight at screen resolution.
"""
import json, os, shutil, struct, sys

out, exp = sys.argv[1], sys.argv[2]
tail = exp.endswith("_tail")
exp = exp.removesuffix("_tail")
W, H = 1920, 1080
if exp.startswith("4k_"):
    W, H, exp = 3840, 2160, exp[3:]
# 16:9 like the layer: a square texture made WE stretch the grain in x
# (WE derives the noise aspect from the layer texture, see measure_grain.py)
TW, TH = 32, 18


def tex_rgba(w, h, px):
    """TEXV0005 / TEXI0001 / TEXB0002, format 0 (RGBA8888), 1 mip, uncompressed."""
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1)
    d += struct.pack("<I", 1)
    d += struct.pack("<5I", w, h, 0, len(px), len(px))
    return d + px


os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "grey.tex"), "wb").write(tex_rgba(TW, TH, bytes((128, 128, 128, 255)) * (TW * TH)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["grey"]}]},
          open(os.path.join(out, "materials", "grey.json"), "w"), indent=1)
json.dump({"material": "materials/grey.json"}, open(os.path.join(out, "models", "grey.json"), "w"))

ASSETS = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
    "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
for name in ("filmgrain", "opacity"):
    src = os.path.join(ASSETS, "effects", name)
    for sub in ("materials", "shaders"):
        shutil.copytree(os.path.join(src, sub), os.path.join(out, sub), dirs_exist_ok=True)
    os.makedirs(os.path.join(out, "effects", name), exist_ok=True)
    shutil.copy(os.path.join(src, "effect.json"), os.path.join(out, "effects", name, "effect.json"))

consts = {}
if exp.startswith("g_s"):
    consts["scale"] = float(exp[3:])
elif exp.startswith("g_a"):
    consts["strength"] = float(exp[3:])
fx = {"file": "effects/filmgrain/effect.json", "id": 10, "visible": True,
      "passes": [{"id": 11, "constantshadervalues": consts}]}
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "grey", "image": "models/grey.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0,
                 "effects": [fx] + ([{"file": "effects/opacity/effect.json", "id": 20, "visible": True,
                                      "passes": [{"id": 21, "constantshadervalues": {"alpha": 1.0}}]}]
                                    if tail else [])}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib filmgrain {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
