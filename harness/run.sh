#!/usr/bin/env bash
# run.sh <workshop id> [seconds=8] [out dir=harness-out/<id>]
#
# Full loop for one wallpaper: Rustpaper render (PNG + video), real WE capture
# (PNG + video), comparison. Open <out>/sheet.png and <out>/side_by_side.mp4.
source "$(dirname "$0")/lib.sh"

id=${1:?workshop id}; secs=${2:-8}; out=${3:-$REPO_DIR/harness-out/$id}
mkdir -p "$out"
bin="$REPO_DIR/target/release/rustpaper-render"
(cd "$REPO_DIR" && cargo build -q --release -p rustpaper-gl --bin rustpaper-render)

"$bin" "$id" --out "$out/rp.png" --time 0 2>"$out/rp_warnings.txt"
# Start well into the animation: WE has been running for a while when its
# capture starts (particles at steady state, not spawning from empty).
"$bin" "$id" --video "$out/rp.mp4" --duration "$secs" --fps 30 --width 1920 --start 10 2>>"$out/rp_warnings.txt"
"$(dirname "$0")/we-capture.sh" "$id" "$out" "$secs"
"$(dirname "$0")/compare.sh" "$out/rp.mp4" "$out/we.mp4" "$out"
sort -u "$out/rp_warnings.txt" | sed 's/^/  /'
echo "-> $out/sheet.png  $out/side_by_side.mp4"
