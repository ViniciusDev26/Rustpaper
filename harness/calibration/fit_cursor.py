#!/usr/bin/env python3
"""fit_cursor.py <probe we.mp4>

Measures the cursor Wallpaper Engine sees from a capture of the
make_depths.py scene (squares at parallaxDepth -0.5, -0.11, 0.12, 0.5, 1.0
along the middle of a 16:9 scene): WE moves each object by
k * depth * (origin - cursor), so over the last second of the video the
squares' x shifts per unit of depth are a line in their rest x whose slope
is k and whose zero is the cursor x; the y shifts give the cursor y.
Prints "x,y k" (cursor in 0..1 of the window, y down). Squares clipped at
the frame edge are left out. No third-party modules needed.
"""
import subprocess, sys

FW, FH = 1920, 1080
DEPTHS = [-0.5, -0.11, 0.12, 0.5, 1.0]
COLORS = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 255)]
REST = [FW * (0.15 + 0.175 * i) for i in range(5)]
SW, SH = 480, 270

path = sys.argv[1]
probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path],
                       capture_output=True, text=True).stdout
start = max(float(probe or 1) - 1.0, 0)
raw = subprocess.run(["ffmpeg", "-v", "error", "-ss", f"{start:.2f}", "-i", path, "-vf",
                      f"fps=10,scale={SW}:{SH}:flags=area,format=rgb24", "-f", "rawvideo", "-"],
                     capture_output=True, check=True).stdout
n = SW * SH * 3
sums = [[0.0, 0.0, 0, SW, 0] for _ in COLORS]  # x, y, count, min x, max x
for f in (raw[i:i + n] for i in range(0, len(raw) - n + 1, n)):
    for p in range(SW * SH):
        r, g, b = f[3 * p], f[3 * p + 1], f[3 * p + 2]
        if max(r, g, b) < 120:
            continue
        k = min(range(5), key=lambda j: sum((a - c) ** 2 for a, c in zip((r, g, b), COLORS[j])))
        if sum((a - c) ** 2 for a, c in zip((r, g, b), COLORS[k])) > 90 ** 2:
            continue
        x = p % SW
        s = sums[k]
        s[0] += x; s[1] += p // SW; s[2] += 1; s[3] = min(s[3], x); s[4] = max(s[4], x)
pts = []
for i, (x, y, c, lo, hi) in enumerate(sums):
    if not c or lo <= 1 or hi >= SW - 2:  # missing or clipped by the frame edge
        continue
    cx, cy = (x / c + 0.5) * FW / SW, (y / c + 0.5) * FH / SH
    pts.append((DEPTHS[i], REST[i], (cx - REST[i]) / DEPTHS[i], (cy - FH / 2) / DEPTHS[i]))
if len(pts) < 3:
    sys.exit("fewer than 3 squares visible: cannot fit")
xs = [p[1] for p in pts]; vs = [p[2] for p in pts]
mx, mv = sum(xs) / len(xs), sum(vs) / len(vs)
k = sum((a - mx) * (b - mv) for a, b in zip(xs, vs)) / sum((a - mx) ** 2 for a in xs)
cursor_x = mx - mv / k
cursor_y = FH / 2 - sum(p[3] for p in pts) / len(pts) / k
print(f"{cursor_x / FW:.3f},{cursor_y / FH:.3f} {k:.3f}")
