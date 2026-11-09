#!/usr/bin/env bash
# run_audio.sh: audio spectrum calibration. Builds the make_audio.py scene,
# captures it in WE while the make_audio_track.py track PLAYS ON THE DEFAULT
# OUTPUT (audible: WE only hears the system output), renders it with
# Rustpaper from the recorded audio, and prints both readings side by side.
source "$(dirname "$0")/../lib.sh"
bin="${RENDER_BIN:-$REPO_DIR/target/release/rustpaper-render}"
d="$REPO_DIR/harness-out/calib/audio"; o="$d/out"; secs=34
python3 "$HARNESS_DIR/calibration/make_audio.py" "$d" >/dev/null
python3 "$HARNESS_DIR/calibration/make_audio_track.py" "$d/track.wav"
mkdir -p "$o"
if [ ! -s "$o/we.mp4" ] || [ "${1:-}" = --recapture ]; then
    WE_PLAY_AUDIO="$d/track.wav" WE_ALLOW_DARK=1 "$HARNESS_DIR/we-capture.sh" "$d" "$o" "$secs" ||
        die "capture failed"
fi
[ -s "$o/we_audio.wav" ] || die "no we_audio.wav (is pw-record installed?)"
"$bin" "$d" --video "$o/rp.mp4" --duration "$secs" --fps 30 --screen 1920x1080 \
    --start "$(capture_time "$o")" --audio "$o/we_audio.wav" 2>"$o/rp_warnings.txt" || die "render failed"
python3 "$HARNESS_DIR/calibration/measure_audio.py" "$o"
