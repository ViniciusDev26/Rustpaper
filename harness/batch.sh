#!/usr/bin/env bash
# batch.sh [--recapture] [--update] [id...]
#
# --update keeps the other rows of results.tsv and re-runs only the given ids.
# Runs the full comparison over many wallpapers and writes
#   harness-out/results.tsv   one row per wallpaper (metrics + verdict)
#   harness-out/report.md     table sorted worst-first + warnings per scene
# Without ids: every scene wallpaper found in the Steam Workshop folder and the
# test corpus. WE captures are reused unless --recapture (they are the slow
# part: ~20 s each, Wallpaper Engine must be running).
source "$(dirname "$0")/lib.sh"
need ffmpeg

recapture=0; update=0
while [ $# -gt 0 ]; do
    case "$1" in
        --recapture) recapture=1; shift ;;
        --update) update=1; shift ;;
        *) break ;;
    esac
done
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
            id=$(basename "$d")
            [ "$t" = scene ] && [[ " ${ids[*]} " != *" $id "* ]] && ids+=("$id")
        done
    done
fi

# documented exclusions (harness/excluded.tsv): never captured nor scored
mapfile -t EXCLUDED < <(grep -v '^#' "$HARNESS_DIR/excluded.tsv" 2>/dev/null | cut -f1)
kept=()
for id in "${ids[@]}"; do
    [[ " ${EXCLUDED[*]} " == *" $id "* ]] && { echo "skip $id: excluded"; continue; }
    kept+=("$id")
done
ids=("${kept[@]}")

results="$OUT/results.tsv"
mkdir -p "$OUT"
header='id\tmean_ssim\tmotion_ssim\tenergy_rp\tenergy_we\tenergy_ratio\tverdict\ttitle\n'
if [ $update = 1 ] && [ -f "$results" ]; then
    keep=$(mktemp)
    awk -F'\t' -v ids=" ${ids[*]} " 'NR > 1 && index(ids, " " $1 " ") == 0' "$results" > "$keep"
    printf "$header" > "$results"; cat "$keep" >> "$results"; rm -f "$keep"
else
    printf "$header" > "$results"
fi
# Captures must be serial (one WE, one headless output); rendering and
# comparing run as background jobs meanwhile. Few jobs while WE is capturing
# (more would steal GPU time from WE and skew its motion), more afterwards.
JOBS_CAPTURE=${BATCH_JOBS_CAPTURE:-3}
JOBS=${BATCH_JOBS:-8}
wait_slots() { while [ "$(jobs -rp | wc -l)" -ge "$1" ]; do wait -n || true; done; }

render_and_compare() { # <id> <dir> <out> <title>
    local id=$1 dir=$2 o=$3 title=$4
    rm -f "$o/row.tsv"
    RUSTPAPER_CLOCK=$(capture_clock "$o" "$secs") \
        "$bin" "$dir" --out "$o/rp.png" --time 10 --screen 1920x1080 2>"$o/rp_warnings.txt" || true
    if ! RUSTPAPER_CLOCK=$(capture_clock "$o" "$secs") \
        "$bin" "$dir" --video "$o/rp.mp4" --duration "$secs" --fps 30 --screen 1920x1080 --start "$(capture_time "$o")" --pointer "$(capture_pointer "$o")" 2>>"$o/rp_warnings.txt"; then
        echo "  render failed: $id"
        printf '%s\t\t\t\t\t\tERROR\t%s\n' "$id" "$title" > "$o/row.tsv"
        return
    fi
    "$HARNESS_DIR/compare.sh" "$o/rp.mp4" "$o/we.mp4" "$o" > /dev/null
    printf '%s\t%s\t%s\n' "$id" "$(cat "$o/metrics.tsv")" "$title" > "$o/row.tsv"
    echo "  done $id: $(cut -f1,6 "$o/metrics.tsv" | tr '\t' ' ')"
}

done_ids=()
for id in "${ids[@]}"; do
    dir=$(wallpaper_dir "$id") || { echo "skip $id: not found"; continue; }
    o="$OUT/$id"; mkdir -p "$o"
    title=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8-sig')).get('title','')[:60])" "$dir/project.json" 2>/dev/null || true)
    echo "== $id $title"
    if [ $recapture = 1 ] || [ ! -s "$o/we.mp4" ]; then
        wait_slots "$JOBS_CAPTURE"
        "$HARNESS_DIR/we-capture.sh" "$id" "$o" "$secs" || { echo "  capture failed"; continue; }
        wait_slots "$JOBS_CAPTURE"
    else
        wait_slots "$JOBS"
    fi
    render_and_compare "$id" "$dir" "$o" "$title" &
    done_ids+=("$id")
done
wait
for id in "${done_ids[@]}"; do
    [ -s "$OUT/$id/row.tsv" ] && cat "$OUT/$id/row.tsv" >> "$results"
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
