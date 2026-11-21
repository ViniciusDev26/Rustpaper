#!/usr/bin/env python3
"""measure_shake.py <video>...: camera path of a make_scene.py coordinate scene.

Every frame of the scene encodes its own scene position (R = x / W, G = y / H),
so the camera offset of a frame is the mean difference between the encoded
position and the pixel's position. Prints, per video, the offset's standard
deviation in x and y (px), the mean speed (px/s) and the share of the motion
energy above 1 Hz (roughness adds a fast component). Frames are 30 fps.
"""
import math, statistics, subprocess, sys

SW, SH, W, H, FPS = 1920, 1080, 1920.0, 1080.0, 30
STEP = 24


def offsets(path):
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-f", "rawvideo",
                          "-pix_fmt", "rgb24", "-"], capture_output=True, check=True).stdout
    n = SW * SH * 3
    out = []
    for f in range(len(raw) // n):
        fr = raw[f * n:(f + 1) * n]
        dx, dy = [], []
        for py in range(64, SH - 32, STEP):  # skip the desktop bar
            for px in range(32, SW - 32, STEP):
                i = (py * SW + px) * 3
                r, g, b = fr[i], fr[i + 1], fr[i + 2]
                if abs(b - 128) > 16 or not (8 < r < 247 and 8 < g < 247):
                    continue
                dx.append(r / 255 * W - px)
                dy.append((1 - g / 255) * H - py)
        if dx:
            out.append((statistics.median(dx), statistics.median(dy)))
    return out


def high_share(xs, cutoff_hz=1.0):
    """Share of the signal's power above cutoff_hz (plain DFT)."""
    m = statistics.mean(xs)
    xs = [x - m for x in xs]
    n = len(xs)
    tot = hi = 0.0
    for k in range(1, n // 2):
        re = sum(x * math.cos(2 * math.pi * k * i / n) for i, x in enumerate(xs))
        im = sum(x * math.sin(2 * math.pi * k * i / n) for i, x in enumerate(xs))
        p = re * re + im * im
        tot += p
        if k * FPS / n > cutoff_hz:
            hi += p
    return hi / tot if tot else 0.0


def corr(a, b):
    n = min(len(a), len(b))
    a, b = a[:n], b[:n]
    ma, mb = statistics.mean(a), statistics.mean(b)
    num = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    den = math.sqrt(sum((x - ma) ** 2 for x in a) * sum((y - mb) ** 2 for y in b))
    return num / den if den else 0.0


def fit_time_scale(xs, ys, t0):
    """Time scale s that best aligns a track with Rustpaper's path model
    x ~ sin(1.05 s t + 0.3), y ~ sin(1.31 s t + 2.1) (t = scene time of each
    30 fps frame from t0): returns (s, corr x, corr y)."""
    best = None
    for i in range(10, 401):
        sc = i / 100
        ts = [(t0 + n / FPS) * sc for n in range(len(xs))]
        cx = corr(xs, [math.sin(t * 1.05 + 0.3) for t in ts])
        cy = corr(ys, [math.sin(t * 1.31 + 2.1) for t in ts])
        if best is None or cx + cy > best[1] + best[2]:
            best = (sc, cx, cy)
    return best


args = sys.argv[1:]
t0 = None
if args[:1] == ["--fit"]:  # --fit <scene time of the first frame>
    t0, args = float(args[1]), args[2:]
paths = args
tracks = {}
for path in paths:
    o = offsets(path)
    xs, ys = [a for a, _ in o], [b for _, b in o]
    tracks[path] = (xs, ys)
    sp = statistics.mean(math.hypot(xs[i + 1] - xs[i], ys[i + 1] - ys[i]) for i in range(len(o) - 1)) * FPS
    print(f"{path}: std x {statistics.pstdev(xs):5.2f} y {statistics.pstdev(ys):5.2f} px   "
          f"half p-p x {(max(xs) - min(xs)) / 2:5.2f} y {(max(ys) - min(ys)) / 2:5.2f}   "
          f"speed {sp:6.1f} px/s   >1Hz x {high_share(xs):.0%} y {high_share(ys):.0%}   ({len(o)} frames)")
    if t0 is not None:
        sc, cx, cy = fit_time_scale(xs, ys, t0)
        print(f"  best time scale {sc:.2f} (corr x {cx:+.2f} y {cy:+.2f})")
if len(paths) == 2:
    (ax, ay), (bx, by) = tracks[paths[0]], tracks[paths[1]]
    print(f"  path correlation (same scene time): x {corr(ax, bx):+.2f} y {corr(ay, by):+.2f}")
