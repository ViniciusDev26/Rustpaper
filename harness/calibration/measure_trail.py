#!/usr/bin/env python3
"""measure_trail.py <video> [<video>...]: trail of the make_motionblur.py bar.

For every frame, the luminance profile p of one row in the middle of the
screen. The blur is an exponential moving average, which CONSERVES the sum
of p (a first try measured the area and saw no trail at all), so the trail
is measured by its shape: the effective width (sum p)^2 / sum p^2 in px
(60 = the bar alone; grows as the bar smears) and the peak brightness.
Prints medians over the frames. Expected effective width of a 60 px bar
moving v px/s under mix(past, current, rate) applied N times per second:
see expected() (numerical, same discretization).
"""
import math, statistics, subprocess, sys

W, H, ROW = 1920, 1080, 600
BAR = 60


def frames(path):
    p = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-vf",
                        f"crop={W}:2:0:{ROW},format=gray", "-f", "rawvideo", "-"],
                       capture_output=True, check=True).stdout
    return [p[i:i + W] for i in range(0, len(p) - 2 * W + 1, 2 * W)]


def eff_width(p):
    s, q = sum(p), sum(x * x for x in p)
    return s * s / q if q else 0.0


def expected(v, rate, n):
    """Effective width of the steady-state trail: a sum of bars shifted by
    v / n px per step with weights rate * (1 - rate)^k."""
    p = [0.0] * W
    for k in range(2000):
        w = rate * (1 - rate) ** k
        if w < 1e-6:
            break
        off = k * v / n
        for x in range(BAR):
            xi = int(round(x + off)) % W
            p[xi] += w * 255
    return eff_width(p)


def speed(fr, fps=30):
    """Bar speed in px/s (the bar moves left): mean step of its left edge."""
    xs = []
    for r in fr:
        e = max(range(W), key=lambda x: r[x])
        n = 0
        while r[(e - 1) % W] >= 128 and n < W:
            e -= 1
            n += 1
        xs.append(e)
    # mean, not median: WE captures step 4 or 5 px per frame (jitter)
    return statistics.mean((xs[i] - xs[i + 1]) % W for i in range(len(xs) - 1)) * fps


def applications(fr, v, rate):
    """Accumulations per second, from the slope of ln(trail): the trail
    behind the bar (to its right) decays by (1 - rate) per application, and
    the bar moves v / N px between applications. Robust where the effective
    width is not: 8-bit accumulation buffers stall faint values."""
    acc = [0.0] * 200
    for r in fr:
        e = max(range(W), key=lambda x: r[x])
        n = 0
        while r[(e + 1) % W] >= 128 and n < W:
            e += 1
            n += 1
        for k in range(200):
            acc[k] += r[(e + 1 + k) % W]
    prof = [a / len(fr) for a in acc]
    pts = [(k, math.log(p)) for k, p in enumerate(prof) if 20 < p < 200]
    if len(pts) < 5:
        return None
    mk = sum(k for k, _ in pts) / len(pts)
    ml = sum(lv for _, lv in pts) / len(pts)
    slope = sum((k - mk) * (lv - ml) for k, lv in pts) / sum((k - mk) ** 2 for k, _ in pts)
    return slope * v / math.log(1 - rate)


if __name__ == "__main__":
    # measure_trail.py [--rate R] video...
    args = sys.argv[1:]
    rate = None
    if args[:1] == ["--rate"]:
        rate, args = float(args[1]), args[2:]
    for path in args:
        fr = frames(path)[30:]
        line = (f"{path}: width {statistics.median(eff_width(r) for r in fr):6.1f} px   "
                f"peak {statistics.median(max(r) for r in fr):5.0f}")
        if rate:
            v = speed(fr)
            n = applications(fr, v, rate)
            line += f"   speed {v:.0f} px/s   accumulations/s {n:.1f}" if n else "   (no trail to fit)"
        print(line + f"   ({len(fr)} frames)")
