#!/usr/bin/env python3
"""make_refract.py <out dir> <experiment>

Calibrates refraction particles (genericparticle REFRACT=1): a background
whose colour encodes its own position (like make_scene.py: R = x, G = y)
under STILL refraction drops (the rain_screen material of 流萤丨崩坏
3576135309: rain_drops_sheet + its normal map, refract amount -0.1). The
drops shift where the background is sampled, and on this background the
colour change is proportional to that shift. measure_refract.py compares a
capture with drops against one without (`_bg`).

Experiments: refr_4k / refr_4k_bg (3840x2160 scene shown at 1080p),
refr_1080 / refr_1080_bg (1920x1080 scene, sizes halved), refr_8k_tiny /
refr_8k_tiny_bg (8192x4096 scene, 400 drops of size 25: ~3 px on screen,
like 3576135309's static drops).
"""
import json, os, struct, sys

out, exp = sys.argv[1], sys.argv[2]
big = exp.startswith("refr_4k")
tiny8k = exp.startswith("refr_8k_tiny")
bg_only = exp.endswith("_bg")
W, H = (8192, 4096) if tiny8k else (3840, 2160) if big else (1920, 1080)
k = 4.27 if tiny8k else 2.0 if big else 1.0
TW, TH = 512, 512


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
os.makedirs(os.path.join(out, "particles"), exist_ok=True)
open(os.path.join(out, "materials", "calib.tex"), "wb").write(tex_rgba(TW, TH, bytes(px)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["calib"]}]},
          open(os.path.join(out, "materials", "calib.json"), "w"), indent=1)
json.dump({"material": "materials/calib.json"}, open(os.path.join(out, "models", "calib.json"), "w"))
json.dump({"passes": [{"alphawriting": "default", "blending": "translucent",
                       "combos": {"CUTOUT": 0, "REFRACT": 1},
                       "constantshadervalues": {"ui_editor_properties_refract_amount": -0.1},
                       "cullmode": "nocull", "depthtest": "disabled", "depthwrite": "disabled",
                       "shader": "genericparticle",
                       "textures": ["particle/water/rain_drops_sheet", "particle/water/rain_drops_sheet_normal"]}]},
          open(os.path.join(out, "materials", "rain_screen.json"), "w"), indent=1)
system = {"material": "materials/rain_screen.json", "maxcount": 400 if tiny8k else 40, "starttime": 5,
          "emitter": [{"name": "boxrandom", "id": 1, "rate": 400 if tiny8k else 40, "minoffset": "0 0 0",
                       "maxoffset": f"{1500 * k} {800 * k} 0"}],
          "initializer": [{"name": "lifetimerandom", "id": 2, "min": 1000, "max": 1000},
                          {"name": "sizerandom", "id": 3, "min": 25 if tiny8k else 150 * k,
                           "max": 25 if tiny8k else 150 * k},
                          {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}],
          "operator": [{"name": "movement", "id": 5}],
          "renderer": [{"name": "sprite", "id": 6}]}
json.dump(system, open(os.path.join(out, "particles", "drops.json"), "w"), indent=1)
objects = [{"id": 1, "name": "calib", "image": "models/calib.json", "origin": f"{W/2} {H/2} 0",
            "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
            "alpha": 1.0, "color": "1 1 1", "brightness": 1.0}]
if not bg_only:
    objects.append({"id": 2, "name": "drops", "particle": "particles/drops.json",
                    "origin": f"{W/2} {H/2} 0", "scale": "1 1 1", "angles": "0 0 0", "visible": True})
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": objects,
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib refract {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
