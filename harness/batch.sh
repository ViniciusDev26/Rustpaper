#!/usr/bin/env bash
# batch.sh [--recapture] [id...]
#
# Runs the full comparison over many wallpapers and writes
#   harness-out/results.tsv   one row per wallpaper (metrics + verdict)
#   harness-out/report.md     table sorted worst-first + warnings per scene
# Without ids: every scene wallpaper found in the Steam Workshop folder and the
# test corpus. WE captures are reused unless --recapture (they are the slow
# part: ~20 s each, Wallpaper Engine must be running).
source "$(dirname "$0")/lib.sh"
need ffmpeg

recapture=0
[ "${1:-}" = "--recapture" ] && { recapture=1; shift; }
secs=${BATCH_SECS:-8}
OUT="$REPO_DIR/harness-out"
bin="$REPO_DIR/target/release/rustpaper-render"
(cd "$REPO_DIR" && cargo build -q --release -p rustpaper-gl --bin rustpaper-render)

ids=("$@")
if [ ${#ids[@]} -eq 0 ]; then
    for root in "$WORKSHOP" "$TESTSET"; do
        [ -d "$root" ] || continue
        for d in "$root"/*/; do
            t=$(python3 -c "import json,sys; print(str(json.load(open(sys.argv[1], encoding='utf-8-sig')).get('type','')).lower())" "$d/project.json" 2>/dev/null || true)
            [ "$t" = scene ] && ids+=("$(basename "$d")")
        done
    done
fi

results="$OUT/results.tsv"
mkdir -p "$OUT"
printf 'id\tmean_ssim\tmotion_ssim\tenergy_rp\tenergy_we\tenergy_ratio\tverdict\ttitle\n' > "$results"
for id in "${ids[@]}"; do
    dir=$(wallpaper_dir "$id") || { echo "skip $id: not found"; continue; }
    o="$OUT/$id"; mkdir -p "$o"
    title=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8-sig')).get('title','')[:60])" "$dir/project.json" 2>/dev/null || true)
    echo "== $id $title"
    if [ $recapture = 1 ] || [ ! -s "$o/we.mp4" ]; then
        "$HARNESS_DIR/we-capture.sh" "$id" "$o" "$secs" || { echo "  capture failed"; continue; }
    fi
    "$bin" "$dir" --out "$o/rp.png" --time 10 2>"$o/rp_warnings.txt" || true
    "$bin" "$dir" --video "$o/rp.mp4" --duration "$secs" --fps 30 --width 1920 --start 10 2>>"$o/rp_warnings.txt" || {
        echo "  render failed"; printf '%s\t\t\t\t\t\tERROR\t%s\n' "$id" "$title" >> "$results"; continue; }
    "$HARNESS_DIR/compare.sh" "$o/rp.mp4" "$o/we.mp4" "$o" > /dev/null
    printf '%s\t%s\t%s\n' "$id" "$(cat "$o/metrics.tsv")" "$title" >> "$results"
done

python3 - "$results" "$OUT" <<'EOF'
import csv, os, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter='\t'))
out = sys.argv[2]
ok = [r for r in rows if r['verdict'] == 'PASS']
def key(r):
    try: return float(r['mean_ssim'])
    except ValueError: return -1
rows.sort(key=key)
with open(os.path.join(out, 'report.md'), 'w') as f:
    f.write(f"# Fidelity report\n\n{len(ok)}/{len(rows)} PASS (mean SSIM >= 0.95 and motion energy ratio 0.7..1.4)\n\n")
    f.write("| id | title | mean SSIM | motion SSIM | energy ratio | verdict | unsupported / warnings |\n|---|---|---|---|---|---|---|\n")
    for r in rows:
        w = os.path.join(out, r['id'], 'rp_warnings.txt')
        warns = sorted({l.strip()[9:90] for l in open(w) if l.startswith('warning')}) if os.path.exists(w) else []
        f.write(f"| {r['id']} | {r['title']} | {r['mean_ssim']} | {r['motion_ssim']} | {r['energy_ratio']} | {r['verdict']} | {'<br>'.join(warns)} |\n")
print(f"{len(ok)}/{len(rows)} PASS -> {os.path.join(out, 'report.md')}")
EOF
