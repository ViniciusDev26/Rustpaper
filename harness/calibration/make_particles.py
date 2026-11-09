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
  trail        spritetrail, velocity 200 up, length 0.007 (ember sparks)
  trail_long   spritetrail, length 0.05, maxlength 100
  size         one still sprite size 200 (rendered size)
  box          boxrandom 600x300, still sprites (distribution)
  sphere_dir   sphererandom 400 with directions "1 0.1 0" (flat)
  oscalpha     oscillatealpha frequencymax 20, scalemin 0.2
  persp        flags 4 (perspective), velocity +-300 in z too
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
elif exp in ("trail", "trail_long"):
    inits.append({"name": "velocityrandom", "id": 5, "min": "-50 200 0", "max": "50 200 0"})
elif exp == "size":
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 1, "instantaneous": 1}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 100, "max": 100},
             {"name": "sizerandom", "id": 3, "min": 200, "max": 200},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp == "box":
    emitter = {"name": "boxrandom", "id": 1, "distancemax": "600 300 0", "rate": 30}
elif exp == "sphere_dir":
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 400, "distancemin": 0, "rate": 30, "directions": "1 0.1 0"}
elif exp == "oscalpha":
    emitter = {"name": "boxrandom", "id": 1, "distancemax": "600 300 0", "rate": 30}
    ops.append({"name": "oscillatealpha", "id": 12, "frequencymax": 20, "scalemin": 0.2})
elif exp == "persp":
    emitter = {"name": "boxrandom", "id": 1, "distancemax": "600 300 600", "rate": 30}
    inits.append({"name": "velocityrandom", "id": 5, "min": "-100 -100 -300", "max": "100 100 300"})
else:
    sys.exit(f"unknown experiment {exp}")

renderer = {"name": "sprite", "id": 20}
if exp == "trail":
    renderer = {"name": "spritetrail", "id": 20, "length": 0.007}
elif exp == "trail_long":
    renderer = {"name": "spritetrail", "id": 20, "length": 0.05, "maxlength": 100}
system = {"maxcount": 400, "starttime": 4, "emitter": [emitter], "initializer": inits, "operator": ops,
          "renderer": [renderer], "material": "materials/calibdot.json"}
if exp == "persp":
    system["flags"] = 4

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
