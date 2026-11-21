#!/usr/bin/env python3
"""make_minify.py <out dir> <experiment>

A 3840x2160 scene (shown on the 1920x1080 screen) with ONE full-scene image
layer and no effects, textured with a 1 px checkerboard: how does WE minify a
4K layer onto a 1080p screen? Correct mipmapping gives flat grey (std ~0),
point/bilinear sampling of the base level gives aliasing (high std).
measure_grain.py reads the result (level, std).

A `_2130` suffix makes the scene 3840x2130 (minified 1.97x, not 2x).
m_redmips_2130 ships red mip levels (are the .tex's own mips used?).

Experiments: m_1mip (the .tex has only the base level), m_mips (the .tex
carries the full mip chain, box-filtered), m_1mip_fx / m_mips_fx (the same
with a neutral `opacity` effect, so the layer goes through its own target).
"""
import json, os, shutil, struct, sys

out, exp = sys.argv[1], sys.argv[2]
W, H = 3840, 2160
if "_2130" in exp:  # Aurora Lake 1275921440's size: a non-integer 1.97x
    H = 2130
TW, TH = W, H
mips = "mips" in exp
fx = "_fx" in exp


def checker(w, h, cell):
    """RGBA checkerboard with `cell` px squares (cell 1 = base level)."""
    row_a = bytearray()
    row_b = bytearray()
    for x in range(w):
        v = 255 if (x // cell) % 2 == 0 else 0
        row_a += bytes((v, v, v, 255))
        row_b += bytes((255 - v, 255 - v, 255 - v, 255))
    px = bytearray()
    for y in range(h):
        px += row_a if (y // cell) % 2 == 0 else row_b
    return bytes(px)


levels = [(TW, TH, checker(TW, TH, 1))]
if mips:
    w, h = TW, TH
    while w > 1 and h > 1:
        w, h = w // 2, h // 2
        # a box-filtered 1 px checkerboard is flat mid grey at every level;
        # `redmips` ships RED mips instead: does WE sample the .tex's own mip
        # levels (red) or generate its own from the base (grey)?
        px = (200, 40, 40, 255) if "redmips" in exp else (128, 128, 128, 255)
        levels.append((w, h, bytes(px) * (w * h)))

d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, TW, TH, TW, TH, 0)
d += b"TEXB0002\0" + struct.pack("<I", 1) + struct.pack("<I", len(levels))
for w, h, px in levels:
    d += struct.pack("<5I", w, h, 0, len(px), len(px)) + px

os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "checker.tex"), "wb").write(d)
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["checker"]}]},
          open(os.path.join(out, "materials", "checker.json"), "w"), indent=1)
json.dump({"material": "materials/checker.json"}, open(os.path.join(out, "models", "checker.json"), "w"))
effects = []
if fx:
    ASSETS = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
        "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
    src = os.path.join(ASSETS, "effects", "opacity")
    for sub in ("materials", "shaders"):
        shutil.copytree(os.path.join(src, sub), os.path.join(out, sub), dirs_exist_ok=True)
    os.makedirs(os.path.join(out, "effects", "opacity"), exist_ok=True)
    shutil.copy(os.path.join(src, "effect.json"), os.path.join(out, "effects", "opacity", "effect.json"))
    effects = [{"file": "effects/opacity/effect.json", "id": 20, "visible": True,
                "passes": [{"id": 21, "constantshadervalues": {"alpha": 1.0}}]}]
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "checker", "image": "models/checker.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0, "effects": effects}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib minify {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
