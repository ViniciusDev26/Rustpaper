#!/usr/bin/env python3
"""measure_parallax.py <video> [video ...]

For a capture of the run_parallax.sh scene (make_scene.py colors: R = x/W,
G = y/H of a 1920x1080 scene), prints every 0.5 s which scene point is at
the screen center, as an offset from the scene center in scene units
(+x right, +y up). Videos side by side, one column pair each.
"""
import subprocess, sys

W, H = 1920, 1080


def centers(path):
    # average a 32x32 block at the screen center, 10 fps
    cmd = ["ffmpeg", "-v", "error", "-i", path, "-vf",
           "fps=10,crop=32:32:944:524,scale=1:1:flags=area,format=rgb24", "-f", "rawvideo", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    out = []
    for i in range(len(raw) // 3):
        r, g = raw[3 * i], raw[3 * i + 1]
        out.append((r / 255 * W - W / 2, g / 255 * H - H / 2))
    return out


cols = [centers(p) for p in sys.argv[1:]]
print("t(s)\t" + "\t".join(f"{p.split('/')[-1]} dx\tdy" for p in sys.argv[1:]))
for i in range(0, max(len(c) for c in cols), 5):
    row = [f"{i / 10:.1f}"]
    for c in cols:
        row += [f"{c[i][0]:+.0f}", f"{c[i][1]:+.0f}"] if i < len(c) else ["-", "-"]
    print("\t".join(row))
