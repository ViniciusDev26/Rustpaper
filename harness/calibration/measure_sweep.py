#!/usr/bin/env python3
"""measure_sweep.py <video>...: the shimmer sweep of a make_shimmer.py scene.

Per 30 fps frame, the mean level of 8 vertical strips across the screen.
Prints the time-averaged level, the peak brightening over the base, the
share of frames where the centre strip is brightened, and the times at
which the centre strip peaks (to compare the sweep period and phase).
"""
import statistics, subprocess, sys

W, H = 1920, 1080
COLS = 8


def series(path):
    p = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-vf",
                        f"crop={W}:200:0:440,scale={COLS}:1:flags=area,format=gray", "-f", "rawvideo", "-"],
                       capture_output=True, check=True).stdout
    return [list(p[i:i + COLS]) for i in range(0, len(p) - COLS + 1, COLS)]


for path in sys.argv[1:]:
    s = series(path)
    base = min(min(r) for r in s)
    mid = [r[COLS // 2] for r in s]
    peaks = [i / 30 for i in range(1, len(mid) - 1)
             if mid[i] >= mid[i - 1] and mid[i] > mid[i + 1] and mid[i] > base + 10]
    lit = sum(m > base + 5 for m in mid) / len(mid)
    print(f"{path}: mean {statistics.mean(statistics.mean(r) for r in s):6.1f}  base {base}  "
          f"peak +{max(mid) - base}  centre lit {lit:.0%}  centre peaks at {[round(t, 2) for t in peaks][:6]} s")
