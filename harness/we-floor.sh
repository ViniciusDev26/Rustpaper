#!/usr/bin/env bash
# we-floor.sh [--recapture] [id...]
#
# Noise floor of the comparison: captures the REAL Wallpaper Engine a second
# time and compares that capture with the first one (harness-out/<id>/we.mp4)
# through the same compare.sh used for Rustpaper. Whatever WE-vs-WE scores is
# the best Rustpaper could ever score on that scene: particle RNG, capture
# jitter, audio, the cursor and the scene clock differ between two runs of WE
# itself. (The second capture starts at another scene time; Rustpaper renders
# in phase with the first one, so the floor is slightly pessimistic for
# time-driven scenes.)
#
# Output: harness-out/<id>/floor/{we.mp4,summary.txt,metrics.tsv,sheet.png}
# and harness-out/floor.tsv (id + the same columns as results.tsv).
# Without ids: every scene that has a cached capture (harness-out/<id>/we.mp4).
# Second captures are reused unless --recapture. Run it alone: rendering jobs
# in parallel steal GPU time from WE and skew its motion.
source "$(dirname "$0")/lib.sh"
need ffmpeg

recapture=0
[ "${1:-}" = --recapture ] && { recapture=1; shift; }
secs=${BATCH_SECS:-8}
OUT="$REPO_DIR/harness-out"

ids=("$@")
if [ ${#ids[@]} -eq 0 ]; then
    for f in "$OUT"/*/we.mp4; do
        id=$(basename "$(dirname "$f")")
        [[ $id =~ ^[0-9]+$ ]] && ids+=("$id")
    done
fi
mapfile -t EXCLUDED < <(grep -v '^#' "$HARNESS_DIR/excluded.tsv" 2>/dev/null | cut -f1)

table="$OUT/floor.tsv"
[ -f "$table" ] || printf 'id\tmean_ssim\tmotion_ssim\tenergy_a\tenergy_b\tenergy_ratio\tverdict\n' > "$table"
for id in "${ids[@]}"; do
    [[ " ${EXCLUDED[*]} " == *" $id "* ]] && continue
    o="$OUT/$id"; f="$o/floor"
    [ -s "$o/we.mp4" ] || { echo "skip $id: no first capture"; continue; }
    echo "== $id"
    if [ $recapture = 1 ] || [ ! -s "$f/we.mp4" ]; then
        mkdir -p "$f"
        # Dark scenes (night city, black backgrounds: 5 of 123 average a
        # luminance of 0-3) trip we-capture's black-window check. The first
        # capture already proved WE loads them, so allow dark frames when it
        # is dark too.
        dark=0
        if [ -s "$o/mean_we.png" ]; then
            lum=$(ffmpeg -v error -i "$o/mean_we.png" -vf "scale=1:1:flags=area,format=gray" -f rawvideo - | od -An -tu1 | tr -d ' ')
            [ "${lum:-255}" -lt 6 ] && dark=1
        fi
        WE_ALLOW_DARK=$([ $dark = 1 ] && echo 1 || echo "${WE_ALLOW_DARK:-0}") \
            "$HARNESS_DIR/we-capture.sh" "$id" "$f" "$secs" >/dev/null || { echo "  capture failed"; continue; }
    fi
    "$HARNESS_DIR/compare.sh" "$f/we.mp4" "$o/we.mp4" "$f" >/dev/null
    # replace this id's row
    grep -v "^$id"$'\t' "$table" > "$table.tmp" || true
    printf '%s\t%s\n' "$id" "$(cat "$f/metrics.tsv")" >> "$table.tmp"
    mv "$table.tmp" "$table"
    echo "  floor $id: $(cut -f1,5,6 "$f/metrics.tsv" | tr '\t' ' ')"
done
echo "-> $table"
