#!/usr/bin/env bash
# we-capture.sh <workshop id> <out dir> [seconds=8]
#
# Captures the REAL Wallpaper Engine rendering a wallpaper:
#   <out>/we.png   one frame (after a warm-up)
#   <out>/we.mp4   <seconds> of video, 30 fps CFR (real capture timestamps)
# Wallpaper Engine must already be running (Steam -> Wallpaper Engine).
source "$(dirname "$0")/lib.sh"
need hyprctl; need grim; need ffmpeg; need nsenter

id=${1:?workshop id}; out=${2:?out dir}; secs=${3:-8}
warmup=${WE_WARMUP:-6}
project="$WORKSHOP/$id/project.json"
[ -f "$project" ] || die "$project not found"
mkdir -p "$out"
title="rpref_$id"

ensure_headless
we_control -control openWallpaper -file "$(wine_path "$project")" \
    -playInWindow "$title" -width 1920 -height 1080
addr=""
for _ in $(seq 1 60); do
    addr=$(window_address "$title"); [ -n "$addr" ] && break; sleep 0.5
done
[ -n "$addr" ] || die "Wallpaper Engine did not open a window for $id"
park_window "$addr" "rpref"
trap 'close_window "$addr"' EXIT

sleep "$warmup"
grim -o "$HEADLESS" "$out/we.png"

# Video: grab frames as fast as the output refreshes, keep real timestamps,
# then resample to 30 fps CFR so it lines up with rustpaper-render --fps 30.
frames="$out/.frames"; rm -rf "$frames"; mkdir -p "$frames"
end=$(( $(date +%s%N) + secs * 1000000000 )); n=0
while [ "$(date +%s%N)" -lt "$end" ]; do
    t=$(date +%s%N)
    grim -t ppm -o "$HEADLESS" "$frames/$(printf '%05d' $n)_$t.ppm"
    n=$((n + 1))
done
list="$frames/list.txt"
ls "$frames" | grep '\.ppm$' | sort | awk -F'[_.]' '
    NR > 1 { printf "file %c%s%c\nduration %.6f\n", 39, prev, 39, ($2 - pt) / 1e9 }
    { prev = $0; pt = $2 }
    END { printf "file %c%s%c\nduration 0.033\n", 39, prev, 39 }' >"$list"
ffmpeg -loglevel error -y -f concat -safe 0 -i "$list" -vf "fps=30,format=yuv420p" \
    -c:v libx264 -crf 16 -preset fast "$out/we.mp4"
rm -rf "$frames"
echo "captured $n frames in ${secs}s -> $out/we.png, $out/we.mp4"
