#!/usr/bin/env python3
"""measure_depths.py <video> [video ...]

For a capture of the make_depths.py scene (1920x1080 frames), prints each
square's screen centroid averaged over the LAST second of the video, and its
offset from where it sits with no parallax (pixels, +x right, +y down; the
scene is 16:9, so its layout maps onto the frame). Videos side by side.
No third-party modules needed (ffmpeg decodes).
"""
import subprocess, sys

FW, FH = 1920, 1080
DEPTHS = [-0.5, -0.11, 0.12, 0.5, 1.0]
COLORS = [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 255)]
REST = [(FW * (0.15 + 0.175 * i), FH / 2) for i in range(5)]
SW, SH = 480, 270  # analysis resolution (4x downscale)


def frames(path):
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                            "-of", "csv=p=0", path], capture_output=True, text=True).stdout
    start = max(float(probe or 1) - 1.0, 0)
    cmd = ["ffmpeg", "-v", "error", "-ss", f"{start:.2f}", "-i", path, "-vf",
           f"fps=10,scale={SW}:{SH}:flags=area,format=rgb24", "-f", "rawvideo", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    n = SW * SH * 3
    return [raw[i:i + n] for i in range(0, len(raw) - n + 1, n)]


def centroids(path):
    sums = [[0.0, 0.0, 0.0] for _ in COLORS]  # x, y, count
    for f in frames(path):
        for p in range(SW * SH):
            r, g, b = f[3 * p], f[3 * p + 1], f[3 * p + 2]
            if max(r, g, b) < 120:
                continue
            k = min(range(len(COLORS)), key=lambda j: sum((a - c) ** 2 for a, c in zip((r, g, b), COLORS[j])))
            if sum((a - c) ** 2 for a, c in zip((r, g, b), COLORS[k])) > 90 ** 2:
                continue
            sums[k][0] += p % SW
            sums[k][1] += p // SW
            sums[k][2] += 1
    out = []
    for x, y, n in sums:
        out.append(((x / n + 0.5) * FW / SW, (y / n + 0.5) * FH / SH) if n else None)
    return out


cols = [centroids(p) for p in sys.argv[1:]]
print("depth\t" + "\t".join(f"{p.split('/')[-2]}/{p.split('/')[-1]} dx\tdy" for p in sys.argv[1:]))
for i, d in enumerate(DEPTHS):
    row = [f"{d:+.2f}"]
    for c in cols:
        if c[i] is None:
            row += ["-", "-"]
        else:
            row += [f"{c[i][0] - REST[i][0]:+.1f}", f"{c[i][1] - REST[i][1]:+.1f}"]
    print("\t".join(row))
