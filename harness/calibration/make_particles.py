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
  child_s1 / child_sx   a STATIC child system (still box cloud, size 60)
               under an invisible parent, object scale 1 / "0.2 1 1": does the
               static child inherit the object's scale? child_neg: scale
               "-0.2 1 1" (a MIRRORED object, like two of 3576135309's drops)
  quad_r0 / quad_r90   ONE big sprite, object scale "0.3 1 1", rotated 0 /
               90 deg: quad squashed in object or screen axes? quad_prot45:
               the particle rotated 45 deg (squash before/after its rotation?)
  srot_0 / srot_90   still box cloud +-500 x +-120, object scale "2 1 1",
               rotated 0 / 90 deg: order of scale vs rotation (Akame fog)
  pre_rate001 / pre_rate1   still dots, rate 2, life 12, starttime 10, with
               instanceoverride rate 0.01 / none: is the prewarm scaled too?
  sph_hist_x / _x003 / _x01 / _xy   400 tiny still dots from sphererandom,
               distance 0..400, directions 1 0 0 / 1 0.03 0 / 1 0.1 0 / 1 1 0
  sphdir_s1 / sphdir_s2   still dots from sphererandom directions "1 0.03 0",
               distance 10..320 (Winter Comfy's shafts), object scale 1 / 2
  drift_pre / drift_pre_s2   dots drifting left 20 u/s, life 20 s, rate
               0.5, starttime 15 (object scale 1 / 2): prewarm age and drift;
               drift_pre_rot: scale 2 and object rotated 0.413 rad
  trail_still / trail_still_min1 / trail_move_min1   still spritetrail dots
               (length 0.01; _min1 adds minlength 1, maxlength 1.5 like the
               rain drops of 3576135309); _move gives velocity 200 up
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
  ov_colorn05 / ov_alpha05   ov_base with instanceoverride colorn 0.5 0.5 0.5
               / alpha 0.5: does WE apply them once (0.5x light) or twice
               (0.25x)? (Mai Sakurajima's Light shafts suggested twice)
  tiny_8k / tiny_8k_s60 / tiny_8k_s4   sprites of size 20 / 60 / 4 (s4:
               ~0.5 px on screen) on an 8192x4096 scene
               (4.3x minified on screen), velocity +-400; tiny_8k_redmips
               uses a disc whose shipped mips are red (are they sampled?);
               _f4 sets header flag 0x4 (rain_drops_sheet* carry it);
               tiny_8k_sheetred: the real rain_drops_sheet_normal sprite
               sheet with RED mip levels (are sheet mips sampled?)
  rv_c100 / rv_noise   one still dot with remapvalue -> velocity: a
               constant (100 0 0) range / the drops' x range -200..200
  spd_none / spd_c3 / spd_c3_f3   one dot, velocity 100 right, with
               remapvalue -> speed fixed at 3 (no flags / flags 3);
               spd_noise_f3: the rain drops' range -5..7, fbmnoise, flags 3;
               spd_r24 / spd_r24_f3: a non-degenerate range 2..4;
               spd_r01_f3 / spd_r1012_f3: ranges 0..1 and 10..12;
               spd_s2: no remap, object scaled 2 (is the velocity scaled?)
  lowrate / lowrate_ov   emitter rate 0.3, lifetime 10, starttime 15,
               maxcount 16, still dots; _ov adds instanceoverride rate 1.87
               (Mai Sakurajima 1982790211's Light shafts); lowrate_big with
               size 600 (are big sprites drawn at WE's size?); lowrate_shaft
               the same with Light shafts' RG88 texture
  ws / ws0     sphere shell r 200, velocity +-200, object scaled "1.5 0.08 1"
               and rotated 0.35 rad; ws sets system flags 1 (world space):
               does WE squash the simulation with the object or only the
               spawn positions?
  ws_grav / ws0_grav   point emitter, gravity "0 -300 0", same object
  ws_turb / ws0_turb   point emitter, turbulence speed 300, same object
               (do operators act in scene units in world space?)
  drag         sparse dots, velocity 400 right, drag 1, lifetime 3: does WE
               weaken drag (and turbulence) by its step factor h < dt?
  turb_ts0 / turb_ts200   turb with timescale 0 (static field) / 200:
               isolates the clock that scrolls the noise (WE-RE-004 1.3)
  ov_size / ov_size_trail   instanceoverride size 0.5 on ov_base / on the
               trail experiment (does WE scale sizes linearly?)
  trail_4k / sprite_4k   trail / ov_base in a 3840x2160 scene with every
               length doubled: identical on screen if WE works in scene units
  sc_none / sc_start07 / sc_end07 / sc_full   still box cloud, lifetime
               4, size 60: no sizechange / starttime 0.7 / endtime 0.7 /
               defaults. Which part of the life does sizechange shrink?
  rare         rate 0.05 (one every 20 s), lifetime 30, size 200, no starttime:
               does WE emit the first particle at once? (capture WE_WARMUP=0)
"""
import shutil, json, os, sys

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
elif exp in ("turb", "turb_mask_x") or exp.startswith("turb_ts"):
    emitter = emit_cloud
    t = {"name": "turbulence", "id": 11, "speedmin": 100, "speedmax": 100, "scale": 0.01}
    if exp == "turb_mask_x":
        t["mask"] = "1 0 0"
    if exp.startswith("turb_ts"):
        t["timescale"] = int(exp[7:])
    ops.append(t)
elif exp == "vel":
    inits.append({"name": "velocityrandom", "id": 5, "min": "-100 -100 0", "max": "100 100 0"})
elif exp in ("trail", "trail_long", "trail_sx", "trail_sy", "ov_size_trail", "trail_4k"):
    inits.append({"name": "velocityrandom", "id": 5, "min": "-50 200 0", "max": "50 200 0"})
elif exp in ("trail_still", "trail_still_min1", "trail_move_min1"):
    # 流萤丨崩坏 3576135309's drops: spritetrail length 0.01, minlength 1,
    # maxlength 1.5. Are STILL trail particles drawn in WE? _move gives them
    # velocity 200 up for comparison.
    emitter = {"name": "boxrandom", "id": 1, "rate": 20, "minoffset": "-600 -300 0", "maxoffset": "600 300 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 2, "max": 2},
             {"name": "sizerandom", "id": 3, "min": 60, "max": 60},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
    if exp == "trail_move_min1":
        inits.append({"name": "velocityrandom", "id": 5, "min": "0 200 0", "max": "0 200 0"})
elif exp in ("child_s1", "child_sx", "child_neg", "sprite_neg"):
    # A parent that draws nothing visible (one dot off-screen far away) with a
    # STATIC child (no event type): a still box cloud of sprites. The object
    # is scaled "0.2 1 1" in child_sx: does the static child inherit it
    # (positions squashed? quads squashed?) like 3576135309's rain drops?
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 0.001, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 1, "max": 1},
             {"name": "sizerandom", "id": 3, "min": 1, "max": 1},
             {"name": "colorrandom", "id": 4, "min": "0 0 0", "max": "0 0 0"}]
elif exp in ("quad_r0", "quad_r90", "quad_prot45"):
    # ONE big still sprite at the centre, object scale "0.3 1 1", rotated 0 /
    # 90 deg: is the quad squashed along the object's local x (then rotated)
    # or along the screen x? (Akame's fog: WE's spread is 0.81x Rustpaper's.)
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 0, "instantaneous": 1}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 1000, "max": 1000},
             {"name": "sizerandom", "id": 3, "min": 600, "max": 600},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
    if exp == "quad_prot45":  # the particle itself rotated 45 deg
        inits.append({"name": "rotationrandom", "id": 6, "min": "0 0 0.785", "max": "0 0 0.785"})
elif exp in ("srot_0", "srot_90"):
    # Akame 1827211233's fog: a NON-uniformly scaled object (3.245 x 2.624)
    # rotated ~90 deg. A still box cloud wide in x (+-500 x +-120), object
    # scale "2 1 1", rotated 0 / 90 deg: in which order do scale and rotation
    # act on the particle positions?
    emitter = {"name": "boxrandom", "id": 1, "rate": 60, "minoffset": "-500 -120 0", "maxoffset": "500 120 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 6, "max": 6},
             {"name": "sizerandom", "id": 3, "min": 30, "max": 30},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp in ("pre_rate001", "pre_rate1"):
    # rain 1444077782's `Light shafts 0`: instanceoverride rate 0.01 with
    # starttime 10. Is the prewarm run at the scaled rate too (nothing ever
    # spawns) or in real time (particles exist, then barely evolve)?
    emitter = {"name": "boxrandom", "id": 1, "rate": 2, "minoffset": "-600 -300 0", "maxoffset": "600 300 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 12, "max": 12},
             {"name": "sizerandom", "id": 3, "min": 60, "max": 60},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp.startswith("sph_hist_"):
    # Spawn-position histogram of sphererandom with `directions`: hundreds of
    # tiny still dots, distance 0..400. sph_hist_x (1 0 0), _x003 (1 0.03 0),
    # _x01 (1 0.1 0), _xy (1 1 0). measure_spawn.py reads the positions.
    dirs = {"x": "1 0 0", "x003": "1 0.03 0", "x01": "1 0.1 0", "xy": "1 1 0"}[exp[len("sph_hist_"):]]
    emitter = {"name": "sphererandom", "id": 1, "distancemin": 0, "distancemax": 400, "rate": 300,
               "directions": dirs}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 1000, "max": 1000},
             {"name": "sizerandom", "id": 3, "min": 8, "max": 8},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp in ("sphdir_s2", "sphdir_s1"):
    # Winter Comfy's light shafts emitter: sphererandom, directions
    # "1 0.03 0", distance 10..320, on an object scaled 2 (s2) or 1 (s1).
    # Still dots, life 20, prewarm 15: the spread of the spawn positions.
    emitter = {"name": "sphererandom", "id": 1, "distancemin": 10, "distancemax": 320, "rate": 2,
               "directions": "1 0.03 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 20, "max": 20},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
elif exp in ("drift_pre", "drift_pre_s2", "drift_pre_rot"):
    # Winter Comfy 2282120494's light shafts drift left for 8-20 s after a
    # 15 s prewarm: are prewarmed particles as old (as far drifted) in WE?
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 0.5, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 20, "max": 20},
             {"name": "sizerandom", "id": 3, "min": 60, "max": 60},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "-20 0 0", "max": "-20 0 0"}]
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
elif exp in ("ws", "ws0"):
    emitter = {"name": "sphererandom", "id": 1, "distancemin": 200, "distancemax": 200, "rate": 30, "directions": "1 1 0"}
    inits = list(base_init) + [{"name": "velocityrandom", "id": 5, "min": "-200 -200 0", "max": "200 200 0"}]
    inits[0] = {"name": "lifetimerandom", "id": 2, "min": 1, "max": 1}
elif exp in ("ws_grav", "ws0_grav", "ws_turb", "ws0_turb"):
    inits = list(base_init)
    inits[0] = {"name": "lifetimerandom", "id": 2, "min": 1.5, "max": 1.5}
    if exp.endswith("grav"):
        ops = [{"name": "movement", "id": 10, "gravity": "0 -300 0"}]
    else:
        ops = [{"name": "movement", "id": 10, "gravity": "0 0 0"},
               {"name": "turbulence", "id": 11, "speedmin": 300, "speedmax": 300, "scale": 0.01}]
elif exp == "drag":
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 1, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 3, "max": 3},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "400 0 0", "max": "400 0 0"}]
    ops = [{"name": "movement", "id": 10, "gravity": "0 0 0", "drag": 1}]
elif exp.startswith("sc_"):
    emitter = emit_cloud
    inits = [{"name": "lifetimerandom", "id": 2, "min": 4, "max": 4},
             {"name": "sizerandom", "id": 3, "min": 60, "max": 60},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
    sc = {"sc_start07": {"starttime": 0.7}, "sc_end07": {"endtime": 0.7}, "sc_full": {},
          "sc_start07_ov": {"starttime": 0.7}, "sc_none_ov": None}.get(exp)
    if sc is not None:
        ops.append({"name": "sizechange", "id": 12, **sc})
    # same question for the other overrides and change / oscillate operators
    extra = {"ac_ov": {"name": "alphachange", "id": 12, "starttime": 0.7},
             "oa_ov": {"name": "oscillatealpha", "id": 12, "scalemin": 0.5},
             "os_ov": {"name": "oscillatesize", "id": 12, "scalemin": 0.5}}.get(exp[3:])
    if extra:
        ops.append(extra)
elif exp.startswith("ov_") or exp == "sprite_4k":
    life = {"ov_count": 4, "ov_sparse": 3, "ov_sparse_rate": 3}.get(exp, 1)
    rate = 1 if exp.startswith("ov_sparse") else 10
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": rate, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": life, "max": life},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "200 0 0", "max": "200 0 0"}]
elif exp.startswith("rv_"):
    # remapvalue -> velocity (operation remap), ONE still dot: rv_c100 has a
    # constant range (100 0 0)..(100 0 0); rv_noise the x range of
    # 3576135309's drops (-200..200) with simplexnoise, inputscale 10.
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 1, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 3, "max": 3},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
    lo, hi = ("100 0 0", "100 0 0") if exp == "rv_c100" else ("-200 0 0", "200 0 0")
    # rv_noise_life6: lifetime 6 (is the noise input life fraction or seconds?)
    # rv_noise_s5: inputscale 5 (how does the scale map to frequency?)
    if exp == "rv_noise_life6":
        inits[0] = {"name": "lifetimerandom", "id": 2, "min": 6, "max": 6}
    ops.append({"name": "remapvalue", "id": 21, "operation": "remap", "output": "velocity",
                "outputrangemin": lo, "outputrangemax": hi,
                "transformfunction": "simplexnoise", "transforminputscale": 5 if exp == "rv_noise_s5" else 10})
elif exp.startswith("spd_"):
    # remapvalue -> speed (流萤丨崩坏 3576135309's rain drops use it): ONE dot
    # at a time, velocity 100 right, plus remapvalue output speed with a
    # constant range (min = max = 3, so the noise does not matter). Measure
    # the dot's speed: 100 (ignored), 300 (multiplier), 3 (absolute)...
    # spd_none: no remapvalue; spd_c3: range 3..3; spd_c3_f3: with flags 3.
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 0, "distancemin": 0, "rate": 1, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 3, "max": 3},
             {"name": "sizerandom", "id": 3, "min": 40, "max": 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "100 0 0", "max": "100 0 0"}]
    if exp not in ("spd_none", "spd_s2"):
        # spd_noise_f3: the drops' own range -5..7 with flags 3
        lo, hi = {"spd_noise": (-5, 7), "spd_r24": (2, 4), "spd_r01": (0, 1),
                  "spd_r1012": (10, 12)}.get(exp.rsplit("_f3", 1)[0], (3, 3))
        rv = {"name": "remapvalue", "id": 20, "output": "speed", "outputrangemin": lo, "outputrangemax": hi,
              "transformfunction": "fbmnoise", "transforminputscale": 8}
        if exp.endswith("_f3"):
            rv["flags"] = 3
        ops.append(rv)
elif exp in ("tiny_8k", "tiny_8k_s60", "tiny_8k_s4", "tiny_8k_redmips", "tiny_8k_redmips_f4", "tiny_8k_sheetred"):
    # 流萤丨崩坏 3576135309: small sprites on an 8192x4096 scene (4.3x
    # minified on screen): how much light / motion does WE keep?
    sz = 60 if exp.endswith("s60") else 4 if exp.endswith("s4") else 20
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 2500, "distancemin": 0, "rate": 60, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 2, "max": 2},
             {"name": "sizerandom", "id": 3, "min": sz, "max": sz},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"},
             {"name": "velocityrandom", "id": 5, "min": "-400 -400 0", "max": "400 400 0"}]
elif exp in ("lowrate", "lowrate_ov", "lowrate_big", "lowrate_shaft"):
    # Light shafts 1 of Mai Sakurajima 1982790211: emitter rate 0.3, long
    # lives, starttime 15, maxcount 16; _ov adds its instanceoverride rate
    # 1.87. Still, separate dots: total brightness counts the live ones.
    emitter = {"name": "sphererandom", "id": 1, "distancemax": 450, "distancemin": 50, "rate": 0.3, "directions": "1 1 0"}
    inits = [{"name": "lifetimerandom", "id": 2, "min": 10, "max": 10},
             {"name": "sizerandom", "id": 3, "min": 600 if exp in ("lowrate_big", "lowrate_shaft") else 40,
              "max": 600 if exp in ("lowrate_big", "lowrate_shaft") else 40},
             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}]
else:
    sys.exit(f"unknown experiment {exp}")
overrides = {"pre_rate001": {"rate": 0.01}, "ov_colorn05": {"colorn": "0.5 0.5 0.5"}, "ov_alpha05": {"alpha": 0.5},
             "lowrate_ov": {"rate": 1.87}, "ov_rate": {"rate": 0.25}, "ov_speed": {"speed": 0.5}, "ov_count": {"count": 0.5}, "ov_count2": {"count": 0.5}, "ov_sparse_rate": {"rate": 0.25},
             "ov_life": {"lifetime": 0.5},
             "ov_size": {"size": 0.5}, "ov_size_trail": {"size": 0.5},
             "sc_start07_ov": {"size": 0.5}, "sc_none_ov": {"size": 0.5},
             "sc_ac_ov": {"alpha": 0.5}, "sc_a_ov": {"alpha": 0.5}, "sc_oa_ov": {"alpha": 0.5}, "sc_os_ov": {"size": 0.5}}.get(exp)

renderer = {"name": "sprite", "id": 20}
if exp in ("trail", "trail_sx", "trail_sy", "ov_size_trail", "trail_4k"):
    renderer = {"name": "spritetrail", "id": 20, "length": 0.007}
elif exp == "trail_long":
    renderer = {"name": "spritetrail", "id": 20, "length": 0.05, "maxlength": 100}
elif exp == "trail_still":
    renderer = {"name": "spritetrail", "id": 20, "length": 0.01}
elif exp in ("trail_still_min1", "trail_move_min1"):
    renderer = {"name": "spritetrail", "id": 20, "length": 0.01, "minlength": 1, "maxlength": 1.5}
system = {"maxcount": 300 if exp.startswith("srot") else 40 if exp.startswith("pre_rate") else 400 if exp.startswith("sph_hist") else 40 if exp.startswith("sphdir") else 16 if exp.startswith("drift_pre") else 1 if exp.startswith(("spd_", "rv_")) else 16 if exp.startswith("lowrate") else 20 if exp.startswith("ov_") else 400, "starttime": 10 if exp.startswith("pre_rate") else 15 if exp.startswith(("lowrate", "drift_pre", "sphdir")) else 0 if exp == "rare" else 4, "emitter": [emitter], "initializer": inits, "operator": ops,
          "renderer": [renderer], "material": "materials/calibdot.json"}
if exp == "persp":
    system["flags"] = 4
if exp in ("ws", "ws_grav", "ws_turb"):
    system["flags"] = 1

if exp.startswith("tiny_8k"):
    W, H = 8192, 4096
if exp.endswith("_4k"):
    W, H = 3840, 1080 * 2
    def dbl(v):
        return " ".join(str(2 * float(x)) for x in v.split()) if isinstance(v, str) else 2 * v
    for i in system["initializer"]:
        if i["name"] in ("sizerandom", "velocityrandom"):
            i["min"], i["max"] = dbl(i["min"]), dbl(i["max"])
    for e in system["emitter"]:
        for k in ("distancemax", "distancemin"):
            if k in e:
                e[k] = dbl(e[k])

os.makedirs(os.path.join(out, "particles"), exist_ok=True)
os.makedirs(os.path.join(out, "materials"), exist_ok=True)
if exp == "tiny_8k_sheetred":
    # The real 4x4 sprite sheet rain_drops_sheet_normal.tex (RGBA, flag 0x4,
    # TEXS block, 7 mips) with mip levels 1.. replaced by solid red: does WE
    # sample a sprite sheet's mips when minified (red) or not?
    import struct as _st
    _A = os.environ.get("WE_ASSETS_DIR", os.path.expanduser(
        "~/.local/share/Steam/steamapps/common/wallpaper_engine/assets"))
    b = open(os.path.join(_A, "materials/particle/water/rain_drops_sheet_normal.tex"), "rb").read()
    j = b.index(b"TEXB0004")
    head = b[:j + 9]
    imgs, ff, mp4, mips = _st.unpack("<4I", b[j + 9:j + 25])
    k = j + 25
    out_b = bytearray(head) + _st.pack("<4I", imgs, ff, mp4, mips)

    for m in range(mips):
        w, h, comp, unc, size = _st.unpack("<5I", b[k:k + 20])
        data = b[k + 20:k + 20 + size]
        k += 20 + size
        if m == 0:
            out_b += _st.pack("<5I", w, h, comp, unc, size) + data
        else:
            px = bytes((220, 30, 30, 255)) * (w * h)
            out_b += _st.pack("<5I", w, h, 0, len(px), len(px)) + px
    out_b += b[k:]  # TEXS block untouched
    open(os.path.join(out, "materials", "sheetred.tex"), "wb").write(bytes(out_b))
    _js = os.path.join(_A, "materials/particle/water/rain_drops_sheet_normal.tex-json")
    if os.path.exists(_js):
        shutil.copy(_js, os.path.join(out, "materials", "sheetred.tex-json"))
if exp.startswith("tiny_8k_redmips"):
    # a 64x64 white disc whose shipped mip levels are RED: are particle
    # textures sampled with the file's mips when minified (red) or not (white)?
    import struct as _st
    def _lvl(n, red):
        px = bytearray()
        for y in range(n):
            for x in range(n):
                inside = (x - n / 2 + 0.5) ** 2 + (y - n / 2 + 0.5) ** 2 < (n / 2) ** 2
                c = ((220, 30, 30) if red else (255, 255, 255)) if inside else (0, 0, 0)
                px += bytes(c + ((255,) if inside else (0,)))
        return bytes(px)
    lv = [(64, _lvl(64, False))] + [(n, _lvl(n, True)) for n in (32, 16, 8, 4, 2, 1)]
    d = b"TEXV0005\0TEXI0001\0" + _st.pack("<7I", 0, 4 if exp.endswith("_f4") else 0, 64, 64, 64, 64, 0)
    d += b"TEXB0002\0" + _st.pack("<I", 1) + _st.pack("<I", len(lv))
    for n, px in lv:
        d += _st.pack("<5I", n, n, 0, len(px), len(px)) + px
    open(os.path.join(out, "materials", "reddisc.tex"), "wb").write(d)
if exp.startswith("child_"):
    child = {"material": "materials/calibdot.json", "maxcount": 200, "starttime": 4,
             "emitter": [{"name": "boxrandom", "id": 1, "rate": 50, "minoffset": "-700 -350 0", "maxoffset": "700 350 0"}],
             "initializer": [{"name": "lifetimerandom", "id": 2, "min": 4, "max": 4},
                             {"name": "sizerandom", "id": 3, "min": 60, "max": 60},
                             {"name": "colorrandom", "id": 4, "min": "255 255 255", "max": "255 255 255"}],
             "operator": [{"name": "movement", "id": 5}], "renderer": [{"name": "sprite", "id": 6}]}
    json.dump(child, open(os.path.join(out, "particles", "child.json"), "w"), indent=1)
    system["children"] = [{"id": 30, "name": "particles/child.json"}]
json.dump(system, open(os.path.join(out, "particles", "calib.json"), "w"), indent=1)
json.dump({"passes": [{"shader": "genericparticle", "blending": "additive", "cullmode": "nocull",
                       "depthtest": "disabled", "depthwrite": "disabled",
                       # lowrate_shaft: Light shafts' own RG88 texture (flags 0x80002)
                       # quad_*: an ELONGATED texture so shape/orientation shows
                       "textures": ["particle/light/light_shafts_6" if exp == "lowrate_shaft" or exp.startswith("quad_")
                                    else "reddisc" if exp.startswith("tiny_8k_redmips")
                                    else "sheetred" if exp == "tiny_8k_sheetred" else "particle/halo"]}]},
          open(os.path.join(out, "materials", "calibdot.json"), "w"), indent=1)
scene = {
    "camera": {"center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0"},
    "general": {"clearcolor": "0 0 0", "clearenabled": True, "orthogonalprojection": {"width": W, "height": H},
                "zoom": 1.0, "bloom": False, "hdr": False, "cameraparallax": False},
    "objects": [{"id": 1, "name": "calib", "particle": "particles/calib.json", "image": None,
                 "origin": f"{W/2} {H/2} 0", "scale": {"trail_sx": "1.5 0.08 1", "trail_sy": "0.2 1 1",
                                                        "sprite_sx": "0.2 1 1", "sprite_s2": "2 2 2", "child_sx": "0.2 1 1",
                                                        "child_neg": "-0.2 1 1", "srot_0": "2 1 1", "srot_90": "2 1 1", "quad_r0": "0.3 1 1", "quad_r90": "0.3 1 1", "quad_prot45": "0.3 1 1", "spd_s2": "2 2 2", "drift_pre_s2": "2 2 2", "drift_pre_rot": "2 2 2", "sphdir_s2": "2 2 2",
                                                        **{k: "1.5 0.08 1" for k in ("ws", "ws0", "ws_grav", "ws0_grav", "ws_turb", "ws0_turb")}}.get(exp, "1 1 1"),
                 "angles": "0 0 0.35" if exp.startswith("ws") else "0 0 0.413" if exp == "drift_pre_rot"
                 else "0 0 1.5708" if exp in ("srot_90", "quad_r90") else "0 0 0",
                 "visible": True,
                 **({"instanceoverride": {"id": 2, **overrides}} if overrides else {})}],
}
json.dump(scene, open(os.path.join(out, "scene.json"), "w"), indent=1)
json.dump({"type": "scene", "file": "scene.json", "title": f"particle calib {exp}"},
          open(os.path.join(out, "project.json"), "w"), indent=1)
print(out)
