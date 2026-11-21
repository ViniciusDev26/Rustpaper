#!/usr/bin/env python3
"""measure.py <capture.png> <scene w> <scene h> [top px to skip]

Fits screen -> scene coordinates from a capture of a calibration scene:
x_scene = ax * px + bx, y_scene = ay * py + by. Prints the fit and the
equivalent "visible scene rectangle" (what part of the scene fills the screen).
"""
import subprocess, sys
png, W, H = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
skip = int(sys.argv[4]) if len(sys.argv) > 4 else 40
SW, SH = 1920, 1080
raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", png, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                     capture_output=True).stdout
def fit(pairs):
    n = len(pairs); sx = sum(p for p, _ in pairs); sy = sum(v for _, v in pairs)
    sxx = sum(p * p for p, _ in pairs); sxy = sum(p * v for p, v in pairs)
    a = (n * sxy - sx * sy) / (n * sxx - sx * sx); return a, (sy - a * sx) / n
xs, ys = [], []
for py in range(skip, SH, 7):
    for px in range(0, SW, 7):
        i = (py * SW + px) * 3
        r, g, b = raw[i], raw[i + 1], raw[i + 2]
        if abs(b - 128) > 12 or not (6 < r < 249 and 6 < g < 249):
            continue  # outside the image (clear color) or saturated edge
        xs.append((px + 0.5, r / 255 * W)); ys.append((py + 0.5, g / 255 * H))
ax, bx = fit(xs); ay, by = fit(ys)
print(f"x = {ax:.4f} * px + {bx:.1f}   y = {ay:.4f} * py + {by:.1f}   ({len(xs)} samples)")
print(f"visible scene: x {bx:.0f} .. {bx + ax * SW:.0f}   y {by + ay * SH:.0f} .. {by:.0f}  "
      f"(scale {1/abs(ax):.4f} px/unit; center {bx + ax * SW / 2:.0f}, {by + ay * SH / 2:.0f})")
