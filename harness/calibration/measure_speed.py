#!/usr/bin/env python3
"""measure_speed.py <video>...: speed of the single moving dot of a
make_particles.py `spd_*` scene: per 30 fps frame the x centroid of the lit
pixels; prints the median per-frame step (excluding respawn jumps) in px/s."""
import statistics, subprocess, sys

W, H = 480, 270


def cx(path):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-vf",
                          f"crop=iw:ih-ih*32/1080:0:ih*32/1080,scale={W}:{H}:flags=area,format=gray",
                          "-f", "rawvideo", "-"], capture_output=True, check=True).stdout
    out = []
    for i in range(0, len(raw) - W * H + 1, W * H):
        f = raw[i:i + W * H]
        s = sx = 0
        for k in range(0, len(f), 2):
            if f[k] > 40:
                s += f[k]
                sx += f[k] * (k % W)
        out.append(sx / s if s else None)
    return out


for path in sys.argv[1:]:
    c = cx(path)
    steps = [b - a for a, b in zip(c, c[1:]) if a is not None and b is not None and -20 < b - a < 20]
    k = 30 * 1920 / W
    v = statistics.median(steps) * k if steps else 0
    a = statistics.mean(abs(x) for x in steps) * k if steps else 0
    print(f"{path}: dot speed {v:7.1f} px/s, mean |speed| {a:7.1f} px/s ({len(steps)} steps)")
