#!/usr/bin/env bash
# capture_parallax.sh <workshop id...>
#
# Captures camera-parallax scenes with the cursor WE actually saw. The
# pointer xpointer.py records can be wrong (a reading clamped to an edge said
# (0, 0.232) while WE's cursor was at (0.24, 0)) and drifts between captures,
# and a parallax comparison needs it exactly. For each scene: capture the
# depth probe (calibration/make_depths.py, delay 0.1 so it settles at once),
# capture the scene, capture the probe again; when both probes put the
# cursor within PROBE_TOL of each other, their mean replaces we_pointer.txt
# (the xpointer reading is kept as we_pointer.xpointer.txt). Otherwise the
# scene is reported and its pointer left as recorded.
source "$(dirname "$0")/lib.sh"
tol=${PROBE_TOL:-0.01}
probe_dir="$REPO_DIR/harness-out/calib/depths_probe"
probe() {
    python3 "$HARNESS_DIR/calibration/make_depths.py" "$probe_dir" 3840 2160 0.4 1.0 0.1 >/dev/null
    WE_ALLOW_DARK=1 WE_WARMUP=4 "$HARNESS_DIR/we-capture.sh" "$probe_dir" "$probe_dir/out" 2 >/dev/null || return 1
    python3 "$HARNESS_DIR/calibration/fit_cursor.py" "$probe_dir/out/we.mp4" | cut -d' ' -f1
}
for id in "$@"; do
    before=$(probe) || { echo "$id: probe failed"; continue; }
    out="$REPO_DIR/harness-out/$id"
    "$HARNESS_DIR/we-capture.sh" "$id" "$out" "${SECS:-10}" >/dev/null || { echo "$id: capture failed"; continue; }
    after=$(probe) || { echo "$id: probe failed"; continue; }
    verdict=$(python3 - "$before" "$after" "$tol" <<'EOF'
import sys
b = [float(v) for v in sys.argv[1].split(",")]
a = [float(v) for v in sys.argv[2].split(",")]
tol = float(sys.argv[3])
if max(abs(x - y) for x, y in zip(a, b)) <= tol:
    print(f"ok {(a[0] + b[0]) / 2:.3f},{(a[1] + b[1]) / 2:.3f}")
else:
    print("moved")
EOF
)
    cp "$out/we_pointer.txt" "$out/we_pointer.xpointer.txt" 2>/dev/null || true
    if [ "${verdict%% *}" = ok ]; then
        echo "${verdict#ok }" > "$out/we_pointer.txt"
        echo "$id: cursor ${verdict#ok } (probes $before / $after, xpointer $(cat "$out/we_pointer.xpointer.txt" 2>/dev/null))"
    else
        echo "$id: cursor MOVED during the capture (probes $before / $after); pointer left as recorded"
    fi
done
