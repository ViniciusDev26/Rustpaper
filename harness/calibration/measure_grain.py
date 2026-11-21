#!/usr/bin/env python3
"""measure_grain.py <video>...: grain statistics of a make_grain.py scene.

Over a 512x256 crop in the middle of the screen (grey, 0-255), per frame:
mean level, spatial std (grain contrast), mean |frame - previous frame|
(what compare.sh's motion energy sees; frames identical to the previous one
are skipped like there), and the share of pixels that changed by more than 4.
Prints medians over the frames (30 fps videos).
"""
import statistics, subprocess, sys

CW, CH, CX, CY = 512, 256, 704, 412


def frames(path):
    p = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", path, "-vf",
                        f"crop={CW}:{CH}:{CX}:{CY},format=gray", "-f", "rawvideo", "-"],
                       capture_output=True, check=True).stdout
    n = CW * CH
    return [p[i:i + n] for i in range(0, len(p) - n + 1, n)]


for path in sys.argv[1:]:
    fr = frames(path)[15:]
    means, stds, diffs, changed = [], [], [], []
    for i, f in enumerate(fr):
        sub = f[::7]
        means.append(statistics.mean(sub))
        stds.append(statistics.pstdev(sub))
        if i:
            d = [abs(a - b) for a, b in zip(f[::7], fr[i - 1][::7])]
            m = statistics.mean(d)
            if m > 0:
                diffs.append(m)
                changed.append(sum(x > 4 for x in d) / len(d))
    print(f"{path}: level {statistics.median(means):5.1f}   grain std {statistics.median(stds):5.2f}   "
          f"|diff| {statistics.median(diffs) if diffs else 0:5.2f}   changed {statistics.median(changed) if changed else 0:.0%}   "
          f"repeats {len(fr) - 1 - len(diffs)}/{len(fr) - 1}")
