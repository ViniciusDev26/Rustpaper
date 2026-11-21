#!/usr/bin/env python3
"""measure_spawn.py <video>...: spawn-position statistics of a
make_particles.py `sph_hist_*` scene (tiny still dots around the screen
centre, 1920x1080 scene). From one frame at 3 s, the lit pixels' offsets from
the centre in scene units: mean |x|, rms x, rms y, and the share of pixels
farther than 200 units in x."""
import math, subprocess, sys

W, H = 1920, 1080


def stats(path):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-ss", "3", "-i", path, "-frames:v", "1",
                          "-vf", "format=gray", "-f", "rawvideo", "-"], capture_output=True, check=True).stdout
    xs, ys = [], []
    for y in range(40, H):
        row = raw[y * W:(y + 1) * W]
        for x in range(W):
            if row[x] > 60:
                xs.append(x - W / 2)
                ys.append(y - H / 2)
    n = len(xs) or 1
    return (n, sum(abs(x) for x in xs) / n, math.sqrt(sum(x * x for x in xs) / n),
            math.sqrt(sum(y * y for y in ys) / n), sum(abs(x) > 200 for x in xs) / n)


for path in sys.argv[1:]:
    n, ax, rx, ry, far = stats(path)
    print(f"{path}: lit {n:6d}  mean|x| {ax:6.1f}  rms x {rx:6.1f}  rms y {ry:6.1f}  |x|>200 {far:.0%}")
