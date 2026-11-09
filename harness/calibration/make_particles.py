#!/usr/bin/env python3
"""make_particles.py <out dir> <experiment>

Builds a loose WE scene with ONE particle system on a black 1920x1080 scene,
to calibrate one particle feature at a time against WE (capture it with
we-capture.sh, render it with rustpaper-render, compare with measure_particles.py).
Particles are small white halos emitted from the scene center.

Experiments (name -> what varies):
  tvr          turbulentvelocityrandom, speed 100, offset 0
  tvr_off3     same with offset 3
  tvr_offm05   same with offset -0.5
  tvr_scale01  same with scale 0.1 (smoother noise)
  turb         turbulence operator (speed 100, scale 0.01) on a still cloud
  turb_mask_x  turbulence with mask "1 0 0"
  vel          velocityrandom +-100 (sanity baseline)
"""
import json, os, sys

out, exp = sys.argv[1], sys.argv[2]
W, H = 1920, 1080

emit_still = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 30, "directions": "1 1 0"}
emit_cloud = {"name": "boxrandom", "id": 1, "distancemax": "600 300 0", "rate": 30}
base_init = [
    {"name": "lifetimerandom", "id": 2, "min": 4, "max": 4},
    {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
    {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
]
tvr = lambda **kw: {"name": "turbulentvelocityrandom", "id": 5, "speedmin": 100, "speedmax": 100, "scale": 1, **kw}
ops = [{"name": "movement", "id": 10, "gravity": "0 0 0"}]
emitter = emit_still
inits = list(base_init)
if exp == "tvr":
    inits.append(tvr())
elif exp == "tvr_off3":
    inits.append(tvr(offset=3))
elif exp == "tvr_offm05":
    inits.append(tvr(offset=-0.5))
elif exp == "tvr_scale01":
    inits.append(tvr(scale=0.1))
elif exp in ("turb", "turb_mask_x"):
    emitter = emit_cloud
    t = {"name": "turbulence", "id": 11, "speedmin": 100, "speedmax": 100, "scale": 0.01}
    if exp == "turb_mask_x":
        t["mask"] = "1 0 0"
    ops.append(t)
elif exp == "vel":
    inits.append({"name": "velocityrandom", "id": 5, "min": "-100 -100 0", "max": "100 100 0"})
else:
    sys.exit(f"unknown experiment {exp}")

system = {"maxcount": 400, "starttime": 4, "emitter": [emitter], "initializer": inits, "operator": ops,
          "renderer": [{"name": "sprite", "id": 20}], "material": "materials/calibdot.json"}

os.makedirs(os.path.join(out, "particles"), exist_ok=True)
os.makedirs(os.path.join(out, "materials"), exist_ok=True)
json.dump(system, open(os.path.join(out, "particles", "calib.json"), "w"), indent=1)
json.dump({"passes": [{"shader": "genericparticle", "blending": "additive", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled", "textures": ["particle/halo"]}]},
          open(os.path.join(out, "materials", "calibdot.json"), "w"), indent=1)
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "calib", "particle": "particles/calib.json", "image": None,
                 "origin": f"{W/2} {H/2} 0", "scale": "1 1 1", "angles": "0 0 0", "visible": True}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"particle calib {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
