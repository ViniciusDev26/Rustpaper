#!/usr/bin/env python3
"""measure_audio.py <capture dir> [video ...]

Reads the make_audio.py cells back from videos (default: we.mp4 and rp.mp4
in the capture dir) and summarizes each segment of the calibration track
(make_audio_track.py), WE next to Rustpaper:

  sweep    peak band (64) and its value per tone frequency
  levels   peak value per 1 kHz amplitude
  decay    peak value 0.1 .. 0.9 s after the loud tone stops
  stereo   left vs right peak for the left-only tone

The track position is found from the recording itself (we_audio.wav): the
first loud tone after silence is the start of the sweep (track t = 1 s).
Values are the captured gray (shader shows 0.25 + 0.75 * band), compared as
is: both videos go through the same encoding.
Pure Python + ffmpeg.
"""
import os, struct, subprocess, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from make_audio_track import segments  # noqa: E402

FPS = 30


def cells(path):
    """Per frame: (left64, right64, left16) values in 0..1."""
    cmd = ["ffmpeg", "-v", "error", "-i", path, "-vf",
           f"fps={FPS},scale=64:9:flags=area,format=gray", "-f", "rawvideo", "-"]
    data = subprocess.run(cmd, capture_output=True, check=True).stdout
    out = []
    for i in range(len(data) // (64 * 9)):
        f = data[i * 576:(i + 1) * 576]
        row = lambda r: [f[r * 64 + c] / 255 for c in range(64)]
        l16 = row(7)
        out.append((row(1), row(4), [(l16[4 * k + 1] + l16[4 * k + 2]) / 2 for k in range(16)]))
    return out


def onset(wav):
    """Start of the first tone: the first 10 ms window louder than -10 dB RMS
    right after 50 ms of silence (other system sounds may precede it)."""
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", wav, "-f", "f32le", "-ac", "1", "-ar", "48000", "-"],
                         capture_output=True, check=True).stdout
    x = struct.unpack(f"<{len(raw) // 4}f", raw)
    rms = lambda a, n: (sum(v * v for v in x[a:a + n]) / max(n, 1)) ** 0.5
    step = 480  # 10 ms
    for i in range(2400, len(x) - step, step):
        if rms(i, step) > 0.3 and rms(i - 2400, 2400) < 0.01:
            # refine to the first sample above -40 dB inside the window
            for j in range(i - step, i + step):
                if abs(x[j]) > 0.01:
                    return j / 48000
            return i / 48000
    return None


def window(frames, t0, t1):
    a, b = max(0, int(t0 * FPS)), min(len(frames), int(t1 * FPS))
    if b <= a:
        return None
    n = b - a
    avg = lambda k: [sum(frames[i][k][c] for i in range(a, b)) / n for c in range(len(frames[a][k]))]
    return avg(0), avg(1), avg(2)


def peak(v):
    i = max(range(len(v)), key=lambda k: v[k])
    return i, v[i]


d = sys.argv[1]
videos = sys.argv[2:] or [os.path.join(d, "we.mp4"), os.path.join(d, "rp.mp4")]
first = onset(os.path.join(d, "we_audio.wav"))
if first is None:
    sys.exit("no tone found in we_audio.wav: was the track playing?")
shift = first - 1.0  # video time of track t = 0
print(f"track starts at video t = {shift:.2f} s")
data = {os.path.basename(v): cells(v) for v in videos}
names = list(data)
t = 0
print("segment\t" + "\t".join(f"{n} peak band\t{n} value" for n in names))
for label, secs, *_ in segments():
    s0 = shift + t
    if label == "silence after loud":
        for k in range(1, 10):
            vals = []
            for n in names:
                w = window(data[n], s0 + k / 10 - 0.02, s0 + k / 10 + 0.02)
                vals += list(peak(w[0])) if w else ["-", "-"]
            print(f"decay +{k / 10:.1f}s\t" + "\t".join(f"{v:.3f}" if isinstance(v, float) else str(v) for v in vals))
    else:
        vals = []
        for n in names:
            w = window(data[n], s0 + 0.4, s0 + 0.9)
            if not w:
                vals += ["-", "-"]
                continue
            b, v = peak(w[0])
            if label == "500 Hz left only":
                vals += [f"L {v:.3f}", f"R {peak(w[1])[1]:.3f}"]
            else:
                vals += [b, f"{v:.3f}"]
        print(f"{label}\t" + "\t".join(str(v) for v in vals))
    t += secs
