#!/usr/bin/env bash
# compare.sh <rustpaper.mp4> <we.mp4> <out dir>
#
# WE's clock can't be controlled, so frame-by-frame diffs are meaningless for
# animated scenes. Instead we compare things that don't depend on phase:
#   mean     temporal average of all frames  -> "what the scene looks like"
#   motion   average |frame - previous frame| -> "where and how much it moves"
# plus a side-by-side video for eyeballing timing. Top 32 px are ignored (the
# desktop bar is drawn over the headless output).
source "$(dirname "$0")/lib.sh"
need ffmpeg

rp=${1:?rustpaper video}; we=${2:?we video}; out=${3:?out dir}
mkdir -p "$out"
# tmix accumulates in 16 bits: keep frames * 255 < 65536 (<= ~250 frames).
secs=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$rp" | cut -d. -f1)
secs=$(( secs < 1 ? 1 : (secs > 16 ? 16 : secs) ))
norm="scale=1920:1080:flags=lanczos,crop=1920:1048:0:32,format=rgb24"

for side in rp we; do
    src=$rp; [ $side = we ] && src=$we
    ffmpeg -loglevel error -y -i "$src" -vf "$norm,fps=5,tmix=frames=$((secs * 5))" -update 1 "$out/mean_$side.png"
    ffmpeg -loglevel error -y -i "$src" -vf "$norm,fps=15,tblend=all_mode=difference,tmix=frames=$((secs * 15)),lutrgb=r=val*8:g=val*8:b=val*8" \
        -update 1 "$out/motion_$side.png"
done

ffmpeg -loglevel error -y -i "$out/mean_we.png" -i "$out/mean_rp.png" \
    -filter_complex "blend=all_mode=difference,lutrgb=r=val*4:g=val*4:b=val*4" "$out/mean_diff.png"

label() { echo "drawtext=text='$1':fontsize=36:fontcolor=yellow:box=1:boxcolor=black@0.6:x=12:y=12"; }
ffmpeg -loglevel error -y -i "$we" -i "$rp" -filter_complex \
    "[0]$norm,$(label 'Wallpaper Engine')[a];[1]$norm,$(label 'Rustpaper')[b];[a][b]hstack,scale=1920:-2,format=yuv420p" \
    -shortest -c:v libx264 -crf 20 -preset fast "$out/side_by_side.mp4"

ffmpeg -loglevel error -y -i "$out/mean_we.png" -i "$out/mean_rp.png" -i "$out/mean_diff.png" \
    -i "$out/motion_we.png" -i "$out/motion_rp.png" -filter_complex \
    "[0]scale=640:-2,$(label 'WE mean')[a];[1]scale=640:-2,$(label 'RP mean')[b];[2]scale=640:-2,$(label 'diff x4')[c];\
[3]scale=640:-2,$(label 'WE motion')[d];[4]scale=640:-2,$(label 'RP motion')[e];color=black:s=640x350[f];\
[a][b][c]hstack=3[top];[d][e][f]hstack=3[bot];[top][bot]vstack" -frames:v 1 "$out/sheet.png"

ssim() { ffmpeg -i "$1" -i "$2" -lavfi ssim -f null - 2>&1 | grep -o 'All:[0-9.]*' | cut -d: -f2; }
{
    echo "mean   SSIM (1 = identical): $(ssim "$out/mean_we.png" "$out/mean_rp.png")"
    echo "motion SSIM (1 = identical): $(ssim "$out/motion_we.png" "$out/motion_rp.png")"
} | tee "$out/summary.txt"
