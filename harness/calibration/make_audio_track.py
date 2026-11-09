#!/usr/bin/env python3
"""make_audio_track.py <out.wav> [--list]

Writes the audio calibration track (48 kHz stereo, 16-bit), played while WE
captures the make_audio.py scene. Segments, 1 s each unless noted:

  0       silence
  1..19   sine sweep, both channels, amplitude 0.5:
          30 45 60 90 125 180 250 350 500 700 1000 1400 2000 2800 4000
          5600 8000 11000 16000 Hz      (frequency -> band mapping)
  20..26  1 kHz, amplitude 1 0.5 0.25 0.125 0.0625 0.03 0.01  (level scale)
  27      silence                       (decay after a loud tone)
  28      500 Hz on the LEFT channel only (channel separation)
  29..30  silence

`--list` prints the segment table (start, end, label) instead of writing.
"""
import math, struct, sys, wave

RATE = 48_000
SWEEP = [30, 45, 60, 90, 125, 180, 250, 350, 500, 700, 1000, 1400, 2000,
         2800, 4000, 5600, 8000, 11000, 16000]
LEVELS = [1, 0.5, 0.25, 0.125, 0.0625, 0.03, 0.01]


def segments():
    """(label, seconds, freq, amp_left, amp_right)"""
    s = [("silence", 1, 0, 0, 0)]
    s += [(f"tone {f} Hz", 1, f, 0.5, 0.5) for f in SWEEP]
    s += [(f"1 kHz amp {a}", 1, 1000, a, a) for a in LEVELS]
    s += [("silence after loud", 1, 0, 0, 0)]
    s += [("500 Hz left only", 1, 500, 0.5, 0)]
    s += [("silence", 2, 0, 0, 0)]
    return s


if __name__ == "__main__":
    if "--list" in sys.argv:
        t = 0
        for label, secs, *_ in segments():
            print(f"{t}\t{t + secs}\t{label}")
            t += secs
        sys.exit()
    w = wave.open(sys.argv[1], "wb")
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(RATE)
    frames = bytearray()
    for _, secs, f, al, ar in segments():
        n = secs * RATE
        for i in range(n):
            # 5 ms fades so segment edges do not click across the spectrum
            fade = min(1.0, i / 240, (n - i) / 240)
            v = math.sin(2 * math.pi * f * i / RATE) * fade if f else 0.0
            frames += struct.pack("<hh", int(32767 * al * v), int(32767 * ar * v))
    w.writeframes(bytes(frames))
    w.close()
