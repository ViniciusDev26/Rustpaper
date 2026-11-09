#!/usr/bin/env python3
"""make_scene.py <out dir> <scene w> <scene h> [eye x] [eye y]

Builds a loose WE scene (project.json, scene.json, model, material, .tex) with
ONE image covering the whole scene whose color encodes the scene position:
R = x / W, G = y / H (y up, like WE scene space), B = 128. Capturing it in WE
and reading the colors back gives the exact screen -> scene mapping WE uses
(harness/calibration/measure.py). No third-party modules needed.
"""
import json, os, struct, sys

out, W, H = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
ex = float(sys.argv[4]) if len(sys.argv) > 4 else 0.0
ey = float(sys.argv[5]) if len(sys.argv) > 5 else 0.0
TW, TH = 512, 512  # texture resolution (stretched over the scene)

def tex_rgba(w, h, px):
    """TEXV0005 / TEXI0001 / TEXB0002, format 0 (RGBA8888), 1 mip, uncompressed."""
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1)          # one image
    d += struct.pack("<I", 1)                           # one mipmap
    d += struct.pack("<5I", w, h, 0, len(px), len(px))  # w, h, lz4=0, sizes
    return d + px

px = bytearray()
for row in range(TH):           # row 0 = top of the image = scene y = H
    g = round(255 * (1 - (row + 0.5) / TH))
    for col in range(TW):
        r = round(255 * (col + 0.5) / TW)
        px += bytes((r, g, 128, 255))

os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
open(os.path.join(out, "materials", "calib.tex"), "wb").write(tex_rgba(TW, TH, bytes(px)))
json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["calib"]}]},
          open(os.path.join(out, "materials", "calib.json"), "w"), indent=1)
json.dump({"material": "materials/calib.json"}, open(os.path.join(out, "models", "calib.json"), "w"))
scene = {
    "camera": {"center": f"{ex} {ey} -1", "eye": f"{ex} {ey} 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "calib", "image": "models/calib.json", "origin": f"{W/2} {H/2} 0",
                 "size": f"{W} {H}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                 "alpha": 1.0, "color": "1 1 1", "brightness": 1.0}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"calib {W}x{H} eye {ex},{ey}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
