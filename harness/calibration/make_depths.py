#!/usr/bin/env python3
"""make_depths.py <out dir> [scene w=3840] [scene h=2160] [amount=0.78] [influence=0.5] [delay=1.92]

Camera parallax by depth: a loose WE scene (black background) with five
solid squares, each a distinct pure color, at parallaxDepth -0.5, -0.11,
0.12, 0.5 and 1.0, spread along the horizontal middle of the scene, camera
parallax on. Capturing it in WE and finding each color's centroid
(measure_depths.py) gives how far WE moves a layer per unit of depth on each
axis, for negative and positive depths, at the cursor WE saw.

Defaults are Chainsaw Man 3577990983's camera parallax (amount 0.78, mouse
influence 0.5, delay 1.92) in its 3840x2160 scene, where the foreground
hands (depths -0.11 / 0.12) sit off WE's. No third-party modules needed.
"""
import json, os, struct, sys

out = sys.argv[1]
W = int(sys.argv[2]) if len(sys.argv) > 2 else 3840
H = int(sys.argv[3]) if len(sys.argv) > 3 else 2160
amount = float(sys.argv[4]) if len(sys.argv) > 4 else 0.78
influence = float(sys.argv[5]) if len(sys.argv) > 5 else 0.5
delay = float(sys.argv[6]) if len(sys.argv) > 6 else 1.92

# (depth, color): pure, well separated colors for the centroid search
SQUARES = [(-0.5, (255, 0, 0)), (-0.11, (0, 255, 0)), (0.12, (0, 0, 255)),
           (0.5, (255, 255, 0)), (1.0, (255, 0, 255))]
SIDE = H // 8  # square side in scene units


def tex_rgba(w, h, px):
    """TEXV0005 / TEXI0001 / TEXB0002, format 0 (RGBA8888), 1 mip, uncompressed."""
    d = b"TEXV0005\0TEXI0001\0" + struct.pack("<7I", 0, 0, w, h, w, h, 0)
    d += b"TEXB0002\0" + struct.pack("<I", 1)
    d += struct.pack("<I", 1)
    d += struct.pack("<5I", w, h, 0, len(px), len(px))
    return d + px


os.makedirs(os.path.join(out, "materials"), exist_ok=True)
os.makedirs(os.path.join(out, "models"), exist_ok=True)
objects = []
for i, (depth, rgb) in enumerate(SQUARES):
    name = f"sq{i}"
    open(os.path.join(out, "materials", f"{name}.tex"), "wb").write(tex_rgba(4, 4, bytes((*rgb, 255)) * 16))
    json.dump({"passes": [{"shader": "genericimage2", "blending": "normal", "cullmode": "nocull",
                           "depthtest": "disabled", "depthwrite": "disabled", "textures": [name]}]},
              open(os.path.join(out, "materials", f"{name}.json"), "w"), indent=1)
    json.dump({"material": f"materials/{name}.json"}, open(os.path.join(out, "models", f"{name}.json"), "w"))
    x = W * (0.15 + 0.175 * i)
    objects.append({"id": i + 1, "name": name, "image": f"models/{name}.json", "origin": f"{x} {H / 2} 0",
                    "size": f"{SIDE} {SIDE}", "scale": "1 1 1", "angles": "0 0 0", "visible": True,
                    "alpha": 1.0, "color": "1 1 1", "brightness": 1.0,
                    "parallaxDepth": f"{depth} {depth}"})
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": True,
                "cameraparallaxamount": amount, "cameraparallaxmouseinfluence": influence,
                "cameraparallaxdelay": delay},
    "objects": objects,
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json",
           "title": f"depth calib {W}x{H} a{amount} i{influence} d{delay}",
           "general": {"properties": {}}},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(json.dumps({"W": W, "H": H, "side": SIDE,
                  "squares": [{"depth": d, "rgb": c, "x": W * (0.15 + 0.175 * i), "y": H / 2}
                              for i, (d, c) in enumerate(SQUARES)]}))
