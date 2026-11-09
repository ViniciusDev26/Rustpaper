#!/usr/bin/env bash
# we-capture.sh <workshop id> <out dir> [seconds=8]
#
# Captures the REAL Wallpaper Engine rendering a wallpaper:
#   <out>/we.png   one frame (after a warm-up)
#   <out>/we.mp4   <seconds> of video, 30 fps CFR (real capture timestamps)
#   <out>/we_audio.wav  the system audio WE heard during the video (when
#                  pw-record exists), aligned with we.mp4 (t = 0 = first frame)
# WE_PLAY_AUDIO=<file> plays that file on the default output during the video
# (audio calibration; it is audible).
# Wallpaper Engine must already be running (Steam -> Wallpaper Engine).
source "$(dirname "$0")/lib.sh"
need hyprctl; need grim; need ffmpeg; need nsenter

id=${1:?workshop id}; out=${2:?out dir}; secs=${3:-8}
warmup=${WE_WARMUP:-6}
project="$(wallpaper_dir "$id" || true)/project.json"
[ -f "$project" ] || die "$project not found"
mkdir -p "$out"
title="rpref_$(basename "$id")"

ensure_headless
user_focus=$(hyprctl activewindow -j | python3 -c "import json,sys; d = json.load(sys.stdin); print('' if d.get('workspace', {}).get('name') == 'rpref' else d.get('address', ''))") # WE's new window grabs focus: give it back
user_monitor=$(focused_monitor); export PARK_MONITOR=$user_monitor
open_window() {
    t_open=$(date +%s.%N)
    we_control -control openWallpaper -file "$(wine_path "$project")" \
        -playInWindow "$title" -width 1920 -height 1080
    addr=""
    for _ in $(seq 1 60); do
        addr=$(window_address "$title"); [ -n "$addr" ] && return 0; sleep 0.5
    done
    return 1
}
# A crashed WE keeps its process alive behind a crash dialog: restart it.
we_crashed && restart_we
if ! open_window; then
    echo "no window for $id: restarting Wallpaper Engine and retrying" >&2
    restart_we
    open_window || die "Wallpaper Engine did not open a window for $id"
fi
park_window "$addr" "rpref" "$user_focus"
# WE's other windows grab focus when this one closes: refocus after it is gone
audio_pid=""; play_pid=""
# (also stop the audio recorder/player if anything fails mid-capture)
trap 'kill $audio_pid $play_pid 2>/dev/null || true; close_window "$addr"; sleep 0.5; refocus "$user_focus" "$user_monitor"' EXIT

sleep "$warmup"
timeout 20 grim -o "$HEADLESS" "$out/we.png" || die "grim could not capture $HEADLESS"
# WE sometimes shows a black window (wallpaper failed to load in it): that is
# not a reference, so fail instead of comparing against it
lum=$(ffmpeg -v error -i "$out/we.png" -vf "scale=1:1:flags=area,format=gray" -f rawvideo - | od -An -tu1 | tr -d ' ')
[ "${lum:-0}" -ge 6 ] || [ "${WE_ALLOW_DARK:-0}" = 1 ] || { rm -f "$out/we.png"; die "Wallpaper Engine showed a black window for $id"; }

# Video: grab frames as fast as the output refreshes, keep real timestamps,
# then resample to 30 fps CFR so it lines up with rustpaper-render --fps 30.
date -Iseconds > "$out/we_start.txt"
# WE reads the cursor even outside its window (clamped to the window), but
# through XWayland: the X pointer, which only moves while the cursor is over
# an X window (xpointer.py). Record what WE sees, for --pointer.
"$HARNESS_DIR/xpointer.py" "$title" > "$out/we_pointer.txt" 2>/dev/null || rm -f "$out/we_pointer.txt"
# Seconds WE's scene clock has run when recording starts (it opens the
# wallpaper fresh): Rustpaper renders from the same time so time-driven
# animations (puppets, paths, pulses) are in phase.
python3 -c "import sys,time; print(f'{time.time() - float(sys.argv[1]):.2f}')" "$t_open" > "$out/we_time.txt"
frames="$out/.frames"; rm -rf "$frames"; mkdir -p "$frames"
# Audio-reactive scenes: record the monitor of the default output (what WE's
# loopback hears) so rustpaper-render --audio replays the same sound.
# Captures are serial, so each file holds only its own scene's audio.
rm -f "$out/we_audio.wav"
if command -v pw-record >/dev/null; then
    t_audio=$(date +%s.%N)
    pw-record -P '{ stream.capture.sink=true node.name=rustpaper-harness }' \
        --rate 48000 --channels 2 "$frames/audio.wav" 2>/dev/null &
    audio_pid=$!
    sleep 0.3 # let the stream start before the first frame
fi
t_video=$(date +%s.%N)
# Calibration only (run_audio.sh): play a known track on the default output
# so WE hears it. Audible on the user's speakers.
if [ -n "${WE_PLAY_AUDIO:-}" ]; then
    pw-play "$WE_PLAY_AUDIO" & play_pid=$!
fi
end=$(( $(date +%s%N) + secs * 1000000000 )); n=0
while [ "$(date +%s%N)" -lt "$end" ]; do
    t=$(date +%s%N)
    timeout 5 grim -t ppm -o "$HEADLESS" "$frames/$(printf '%05d' $n)_$t.ppm"
    n=$((n + 1))
done
# (|| true: under set -e a kill of an already finished player would abort)
if [ -n "$play_pid" ]; then kill "$play_pid" 2>/dev/null || true; wait "$play_pid" 2>/dev/null || true; fi
if [ -n "$audio_pid" ]; then
    # TERM, not INT: background jobs of a non-interactive shell ignore SIGINT
    kill -TERM "$audio_pid" 2>/dev/null || true; wait "$audio_pid" 2>/dev/null || true
    off=$(python3 -c "import sys; print(f'{float(sys.argv[2]) - float(sys.argv[1]):.3f}')" "$t_audio" "$t_video")
    ffmpeg -loglevel error -y -ss "$off" -t "$secs" -i "$frames/audio.wav" "$out/we_audio.wav" ||
        rm -f "$out/we_audio.wav"
fi
list="$frames/list.txt"
ls "$frames" | grep '\.ppm$' | sort | awk -F'[_.]' '
    NR > 1 { printf "file %c%s%c\nduration %.6f\n", 39, prev, 39, ($2 - pt) / 1e9 }
    { prev = $0; pt = $2 }
    END { printf "file %c%s%c\nduration 0.033\n", 39, prev, 39 }' >"$list"
ffmpeg -loglevel error -y -f concat -safe 0 -i "$list" -vf "fps=30,format=yuv420p" \
    -c:v libx264 -crf 16 -preset fast "$out/we.mp4"
rm -rf "$frames"
echo "captured $n frames in ${secs}s -> $out/we.png, $out/we.mp4"
