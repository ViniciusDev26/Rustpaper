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
  trail_sx     the trail experiment on an object scaled "1.5 0.08 1"
  trail_sy     the trail experiment on an object scaled "0.2 1 1"
  sprite_sx    still sprites (size 40, box cloud) on an object scaled "0.2 1 1"
  sprite_s2    the same on an object scaled "2 2 2" (uniform)
  (object-scale experiments: does WE squash / scale the quads with the
  object's scale, or only move the particles?)
  ov_base      reference for the ov_* runs: velocity 200 right, lifetime 1,
               rate 10, maxcount 20 (uncapped: about 10 alive)
  ov_rate      ov_base with instanceoverride rate 0.25: does it slow the whole
               simulation (WE-RE-002 1.2) or only the emission?
  ov_speed     ov_base with instanceoverride speed 0.5 (initial velocity?)
  ov_count     ov_base, lifetime 4 (capped at 20), instanceoverride count
               0.5: does count scale maxcount or the rate?
  ov_life      ov_base with instanceoverride lifetime 0.5
  ov_count2    ov_base (uncapped) with count 0.5: fewer particles if count
               scales the rate
  ov_sparse    rate 1, lifetime 3, velocity 200 right: separate dots whose
               motion the metric can see
  ov_sparse_rate  ov_sparse with instanceoverride rate 0.25 (slower dots?)
  rare         rate 0.05 (one every 20 s), lifetime 30, size 200, no starttime:
               does WE emit the first particle at once? (capture WE_WARMUP=0)
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
elif exp in ("trail", "trail_long", "trail_sx", "trail_sy"):
    inits.append({"name": "velocityrandom", "id": 5, "min": "-50 200 0", "max": "50 200 0"})
elif exp == "size":
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 1, "instantaneous": 1}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 100, "max": 100},
             {"name": "sizerandom", "id": 3, "min": 200, "max": 200},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp == "rare":
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 0.05}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 30, "max": 30},
             {"name": "sizerandom", "id": 3, "min": 200, "max": 200},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp in ("sprite_sx", "sprite_s2"):
    emitter = {"name": "boxrandom", "id": 1, "distancemax": "600 300 0", "rate": 30}
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
elif exp.startswith("ov_"):
    life = {"ov_count": 4, "ov_sparse": 3, "ov_sparse_rate": 3}.get(exp, 1)
    rate = 1 if exp.startswith("ov_sparse") else 10
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": rate, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": life, "max": life},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "200 0 0", "max": "200 0 0"}]
else:
    sys.exit(f"unknown experiment {exp}")
overrides = {"ov_rate": {"rate": 0.25}, "ov_speed": {"speed": 0.5}, "ov_count": {"count": 0.5}, "ov_count2": {"count": 0.5}, "ov_sparse_rate": {"rate": 0.25},
             "ov_life": {"lifetime": 0.5}}.get(exp)

renderer = {"name": "sprite", "id": 20}
if exp in ("trail", "trail_sx", "trail_sy"):
    renderer = {"name": "spritetrail", "id": 20, "length": 0.007}
elif exp == "trail_long":
    renderer = {"name": "spritetrail", "id": 20, "length": 0.05, "maxlength": 100}
system = {"maxcount": 20 if exp.startswith("ov_") else 400, "starttime": 0 if exp == "rare" else 4, "emitter": [emitter], "initializer": inits, "operator": ops,
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
                 "origin": f"{W/2} {H/2} 0", "scale": {"trail_sx": "1.5 0.08 1", "trail_sy": "0.2 1 1",
                                                        "sprite_sx": "0.2 1 1", "sprite_s2": "2 2 2"}.get(exp, "1 1 1"),
                 "angles": "0 0 0", "visible": True,
                 **({"instanceoverride": {"id": 2, **overrides}} if overrides else {})}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"particle calib {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
