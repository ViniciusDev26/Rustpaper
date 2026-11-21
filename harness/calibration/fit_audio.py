#!/usr/bin/env python3
"""fit_audio.py <capture dir>

Fits the audio analyzer model (rustpaper_gl::audio) to a run_audio.sh
capture: replays we_audio.wav through candidate analyzers and compares the
64 left bands, frame by frame, with what WE showed (we.mp4 cells).

Model (what the measurements pointed to, see docs/GL_RENDERER.md):
  * band k covers log-interpolated frequencies between measured anchors
    (tone frequency -> peak band, from the sweep);
  * linear peak magnitude per band (Hann window, 2048 samples);
  * divided by an adaptive reference: ref rises toward the frame's loudest
    band by `attack` per step and otherwise decays by `release` per step;
  * minus a small `floor`, no temporal smoothing of the bands themselves;
  * shown `latency` seconds late.
Pure Python (slow FFT, cached per run).
"""
import cmath, math, os, struct, subprocess, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
RATE, N, STEP = 48_000, 2048, 1 / 60

# (frequency Hz, band) from the WE sweep (run_audio.sh, 2026-11-09)
ANCHORS = [(45, 0), (90, 1), (125, 2), (180, 5), (250, 8), (350, 13), (500, 20),
           (700, 29), (1000, 32), (1400, 35), (2000, 39), (2800, 43), (4000, 47),
           (5600, 51), (8000, 56), (11000, 61)]


def band_center(k):
    """Center frequency of band k: log-linear between anchors, extrapolated."""
    pts = ANCHORS
    for (f0, b0), (f1, b1) in zip(pts, pts[1:]):
        if k <= b1 or (f1, b1) == pts[-1]:
            t = (k - b0) / (b1 - b0)
            return math.exp(math.log(f0) + t * (math.log(f1) - math.log(f0)))
    return pts[-1][0]


def edges():
    c = [band_center(k) for k in range(64)]
    e = [c[0] / math.sqrt(c[1] / c[0])]
    e += [math.sqrt(a * b) for a, b in zip(c, c[1:])]
    e.append(c[-1] * math.sqrt(c[-1] / c[-2]))
    return [min(max(int(f / RATE * N), 1), N // 2 - 1) for f in e]


def fft(x):
    n = len(x)
    if n == 1:
        return x
    ev, od = fft(x[0::2]), fft(x[1::2])
    t = [cmath.exp(-2j * math.pi * k / n) * od[k] for k in range(n // 2)]
    return [ev[k] + t[k] for k in range(n // 2)] + [ev[k] - t[k] for k in range(n // 2)]


def band_mags(wav, cache):
    if os.path.exists(cache):
        return [list(map(float, l.split())) for l in open(cache)]
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", wav, "-f", "f32le", "-ac", "2", "-ar", str(RATE), "-"],
                         capture_output=True, check=True).stdout
    x = struct.unpack(f"<{len(raw) // 4}f", raw)[0::2]  # left
    win = [0.5 - 0.5 * math.cos(2 * math.pi * i / (N - 1)) for i in range(N)]
    ed = edges()
    out = []
    steps = int(len(x) / RATE / STEP)
    for s in range(steps):
        end = int((s + 1) * STEP * RATE)
        seg = x[max(0, end - N):end]
        seg = [0.0] * (N - len(seg)) + list(seg)
        X = fft([complex(seg[i] * win[i]) for i in range(N)])
        mags = []
        for b in range(64):
            a, z = ed[b], ed[b + 1]
            fc = band_center(b) / RATE * N
            i0 = min(int(fc), N // 2 - 2)
            interp = abs(X[i0]) + (abs(X[i0 + 1]) - abs(X[i0])) * (fc - i0)
            inside = max((abs(X[i]) for i in range(a, z)), default=0.0)
            mags.append(max(inside, interp) / (N / 4))
        out.append(mags)
    with open(cache, "w") as f:
        for m in out:
            f.write(" ".join(f"{v:.5f}" for v in m) + "\n")
    return out


def spread(m, lo, hi):
    """Dilation across bands: band k also shows lo[i] / hi[i] of the band
    i + 1 below / above it (WE's tones light their neighbours)."""
    out = list(m)
    for k in range(64):
        for i, w in enumerate(lo):
            if k + i + 1 < 64:
                out[k] = max(out[k], m[k + i + 1] * w)
        for i, w in enumerate(hi):
            if k - i - 1 >= 0:
                out[k] = max(out[k], m[k - i - 1] * w)
    return out


def simulate(mags, attack, release, floor, ref_min=0.01, lo=(), hi=()):
    ref, out = ref_min, []
    for m in mags:
        m = spread(m, lo, hi) if lo or hi else m
        peak = max(m)
        ref = ref + (peak - ref) * attack if peak > ref else max(ref * release, ref_min)
        out.append([min(max(v / ref - floor, 0.0) / (1 - floor), 1.0) for v in m])
    return out


if __name__ == "__main__":
    d = sys.argv[1]
    src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "measure_audio.py")).read()
    g = {"__file__": os.path.join(os.path.dirname(os.path.abspath(__file__)), "measure_audio.py")}
    exec(src[:src.index("d = sys.argv[1]")], g)
    we = g["cells"](os.path.join(d, "we.mp4"))
    lo, hi = 0.263, 0.976  # captured gray of band value 0 and 1 (shader floor .25)
    target = [[(v - lo) / (hi - lo) for v in f[0]] for f in we]
    mags = band_mags(os.path.join(d, "we_audio.wav"), os.path.join(d, "band_mags.txt"))
    best = None
    for lat in (0.1, 0.133, 0.167, 0.2, 0.233):
        for attack in (0.02, 0.05, 0.1, 0.2, 0.5, 1.0):
            for rel in (0.99, 0.995, 0.998, 0.999):
                for floor in (0.0, 0.02, 0.05):
                    sim = simulate(mags, attack, rel, floor)
                    err, n = 0.0, 0
                    for i, row in enumerate(target):
                        s = int((i / 30 - lat) / STEP)
                        if 0 <= s < len(sim):
                            err += sum((a - b) ** 2 for a, b in zip(row, sim[s]))
                            n += 64
                    e = (err / n) ** 0.5
                    if best is None or e < best[0]:
                        best = (e, lat, attack, rel, floor)
    print("rmse %.4f latency %.3f attack %.2f release %.4f floor %.2f" % best)
