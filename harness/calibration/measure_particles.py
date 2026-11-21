#!/usr/bin/env python3
"""measure_particles.py <video.mp4> [<video.mp4> ...]

Statistics of a particle-calibration capture (make_particles.py), per video:
  total   mean brightness (how much is drawn)
  cx, cy  centroid of the time-averaged image, in scene-relative units
          (0,0 = center, +y = up, 1 = half the screen width / height)
  sx, sy  spread (standard deviation) of that image
  up      share of the brightness above the center (0.5 = symmetric)
  motion  mean |frame - previous frame| at 10 fps (how much it moves)
The top 32 px (the desktop bar on the headless output) are ignored.
Pure Python: frames are scaled down to 192x105 gray.
"""
import subprocess, sys

W, H = 192, 105

def frames(path):
    cmd = ["ffmpeg", "-v", "error", "-i", path, "-vf",
           f"crop=iw:ih-ih*32/1080:0:ih*32/1080,scale={W}:{H}:flags=area,fps=10,format=gray",
           "-f", "rawvideo", "-"]
    data = subprocess.run(cmd, capture_output=True, check=True).stdout
    n = len(data) // (W * H)
    return [data[i * W * H:(i + 1) * W * H] for i in range(n)]

def stats(path):
    fs = frames(path)
    acc = [0.0] * (W * H)
    motion = 0.0
    for i, f in enumerate(fs):
        for k, v in enumerate(f):
            acc[k] += v
        if i:
            prev = fs[i - 1]
            motion += sum(abs(a - b) for a, b in zip(f, prev)) / (W * H)
    n = len(fs)
    acc = [a / n for a in acc]
    tot = sum(acc) or 1.0
    cx = sum(acc[y * W + x] * x for y in range(H) for x in range(W)) / tot
    cy = sum(acc[y * W + x] * y for y in range(H) for x in range(W)) / tot
    sx = (sum(acc[y * W + x] * (x - cx) ** 2 for y in range(H) for x in range(W)) / tot) ** 0.5
    sy = (sum(acc[y * W + x] * (y - cy) ** 2 for y in range(H) for x in range(W)) / tot) ** 0.5
    up = sum(acc[y * W + x] for y in range(H // 2) for x in range(W)) / tot
    return {
        "total": tot / (W * H),
        "cx": (cx - W / 2) / (W / 2), "cy": -(cy - H / 2) / (H / 2),
        "sx": sx / (W / 2), "sy": sy / (H / 2),
        "up": up, "motion": motion / max(n - 1, 1),
    }

for p in sys.argv[1:]:
    s = stats(p)
    print(p, " ".join(f"{k}={v:.3f}" for k, v in s.items()))
