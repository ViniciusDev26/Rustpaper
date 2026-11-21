#!/usr/bin/env python3
"""make_compose.py <out dir> <experiment>

A 3840x2160 coordinate-encoding background (R = x, G = y, see make_scene.py)
under ONE compose layer (models/util/composelayer.json) with a neutral
`opacity` effect, scaled "1.03 0.71482 1" and rotated 0.00646 rad like
こきん 3146507587's object 205. Reading the captured colours back tells how
WE maps what a compose layer samples onto where it draws.
Experiments: cmp_scaled (as above), cmp_plain (scale 1, no rotation),
cmp_grain_last (plain compose with ONLY filmgrain), cmp_grain (filmgrain
then the neutral opacity: grain in an intermediate pass, like compose 22);
a `_s30` suffix uses compose 22's grain constants (scale 30, strength 0.7);
cmp_grain_shine_s30: grain followed by the multi-pass `shine` effect.
"""
import json, os, shutil, struct, sys

out, exp = sys.argv[1], sys.argv[2]
W, H, TW, TH = 3840, 2160, 512, 512


def tex_rgba(w, h, px):
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1) + struct.pack("<I", 1)
    return d + struct.pack("<5I", w, h, 0, len(px), len(px)) + px


px = bytearray()
for row in range(TH):
    g = round(255 * (1 - (row + 0.5) / TH))
    for col in range(TW):
        px += bytes((round(255 * (col + 0.5) / TW), g, 128, 255))
os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "calib.tex"), "wb").write(tex_rgba(TW, TH, bytes(px)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["calib"]}]},
          open(os.path.join(out, "materials", "calib.json"), "w"), indent=1)
json.dump({"material": "materials/calib.json"}, open(os.path.join(out, "models", "calib.json"), "w"))
ASSETS = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
    "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
for name in ("opacity", "filmgrain", "shine"):
    src = os.path.join(ASSETS, "effects", name)
    for sub in ("materials", "shaders"):
        shutil.copytree(os.path.join(src, sub), os.path.join(out, sub), dirs_exist_ok=True)
    os.makedirs(os.path.join(out, "effects", name), exist_ok=True)
    shutil.copy(os.path.join(src, "effect.json"), os.path.join(out, "effects", name, "effect.json"))
scaled = exp == "cmp_scaled"
objects = [
    {"id": 1, "name": "calib", "image": "models/calib.json", "origin": f"{W/2} {H/2} 0",
     "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
     "alpha": 1.0, "color": "1 1 1", "brightness": 1.0},
    {"id": 2, "name": "compose", "image": "models/util/composelayer.json",
     "origin": "1920 1020.32031 0" if scaled else f"{W/2} {H/2} 0", "size": f"{W} {H}",
     "scale": "1.03 0.71482 1" if scaled else "1 1 1", "angles": "0 0 0.00646" if scaled else "0 0 0",
     "visible": True,
     "effects": ([{"file": "effects/filmgrain/effect.json", "id": 30, "visible": True,
                   "passes": [{"id": 31, "constantshadervalues":
                               {"scale": 30, "strength": 0.7, "exponent": 0.5} if exp.endswith("_s30") else {}}]}]
                 if exp.startswith("cmp_grain") else [])
                + ([] if exp.startswith("cmp_grain_last") else
                   [{"file": "effects/shine/effect.json", "id": 40, "visible": True}]
                   if exp.startswith("cmp_grain_shine") else
                   [{"file": "effects/opacity/effect.json", "id": 20, "visible": True,
                     "passes": [{"id": 21, "constantshadervalues": {"alpha": 1.0}}]}])},
]
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": objects,
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib compose {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
