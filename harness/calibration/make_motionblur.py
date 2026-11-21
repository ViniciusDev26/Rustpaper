#!/usr/bin/env python3
"""make_motionblur.py <out dir> <experiment>

Builds a loose WE scene to calibrate the `motionblur` effect alone: one
full-screen image layer (black, with one white vertical bar 1/32 of the
width) scrolled to the right by the built-in `scroll` effect, then
`motionblur`. The bar leaves a trail whose length is set by how often WE
applies the accumulation `mix(past, current, rate)` (measure_trail.py).

Experiments: mb_none (scroll only), mb_<rate> e.g. mb_0.8, mb_0.3, mb_0.13
(constant by its label, as the editor writes it), mbr_<rate> (constant by
its material name `rate`), lbl_scroll (scroll speed 0.5 keyed by its
label, no blur).
"""
import json, os, shutil, struct, sys

out, exp = sys.argv[1], sys.argv[2]
W, H = 1920, 1080
TW, TH = 256, 16
BAR = TW // 32


def tex_rgba(w, h, px):
    """TEXV0005 / TEXI0001 / TEXB0002, format 0 (RGBA8888), 1 mip, uncompressed."""
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1)
    d += struct.pack("<I", 1)
    d += struct.pack("<5I", w, h, 0, len(px), len(px))
    return d + px


px = bytearray()
for _ in range(TH):
    for col in range(TW):
        v = 255 if col < BAR else 0
        px += bytes((v, v, v, 255))

os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "bar.tex"), "wb").write(tex_rgba(TW, TH, bytes(px)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["bar"]}]},
          open(os.path.join(out, "materials", "bar.json"), "w"), indent=1)
json.dump({"material": "materials/bar.json"}, open(os.path.join(out, "models", "bar.json"), "w"))

# The editor packs every effect's materials and shaders into the wallpaper
# (rain 1444077782's scene.pkg has materials/effects/motionblur_*.json):
# copy them from the base assets the same way.
ASSETS = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
    "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
for fx in ("scroll", "motionblur"):
    src = os.path.join(ASSETS, "effects", fx)
    for sub in ("materials", "shaders"):
        if os.path.isdir(os.path.join(src, sub)):
            shutil.copytree(os.path.join(src, sub), os.path.join(out, sub), dirs_exist_ok=True)
    os.makedirs(os.path.join(out, "effects", fx), exist_ok=True)
    shutil.copy(os.path.join(src, "effect.json"), os.path.join(out, "effects", fx, "effect.json"))

# 0.25 texture widths per second = 480 px/s on screen (8 px per frame at 60 fps)
# lbl_scroll: the scroll speed keyed by its LABEL (as many scenes store it,
# e.g. rain 1444077782's waterripple): does WE match constants by label too?
scroll_consts = ({"ui_editor_properties_speed_x": 0.5, "ui_editor_properties_speed_y": 0.0}
                 if exp == "lbl_scroll" else {"speedx": 0.25, "speedy": 0.0})
effects = [{"file": "effects/scroll/effect.json", "id": 10, "visible": True,
            "passes": [{"id": 11, "constantshadervalues": scroll_consts}]}]
if exp not in ("mb_none", "lbl_scroll"):
    rate = float(exp.split("_", 1)[1])
    # mb_: the form the editor writes (rain 1444077782), constant by its label
    key = "rate" if exp.startswith("mbr_") else "Accumulation rate"
    effects.append({"file": "effects/motionblur/effect.json", "id": 20, "visible": True,
                    "passes": [{"id": 21, "constantshadervalues": {key: rate},
                                "textures": [None, "_rt_FullCompoBuffer1_bar_20"]},
                               {"id": 22}]})
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "bar", "image": "models/bar.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0, "effects": effects}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib motionblur {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
