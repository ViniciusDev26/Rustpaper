#!/usr/bin/env python3
"""compare_runs.py [--gate] [old.tsv [new.tsv]]: what changed between two harness runs.

Without arguments: the two most recent files in harness-out/runs/ (batch.sh
saves one per run). With one: that run against the most recent. Prints the
score per SceneScript group for both, verdict flips, mean SSIM moves larger
than --min (default 0.005), and scenes present in only one of them."""
import argparse, glob, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from score import GROUPS, OUT, group, load, skipped  # noqa: E402


def ssim(r):
    try:
        return float(r["mean_ssim"])
    except (KeyError, ValueError):
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("runs", nargs="*")
    ap.add_argument("--min", type=float, default=0.005, help="smallest mean SSIM change to list")
    ap.add_argument("--gate", action="store_true",
                    help="exit 1 on a regression: a PASS -> FAIL flip or a mean SSIM drop >= --drop")
    ap.add_argument("--drop", type=float, default=0.01, help="mean SSIM drop that fails --gate")
    a = ap.parse_args()
    saved = sorted(glob.glob(os.path.join(OUT, "runs", "*.tsv")))
    if len(a.runs) == 2:
        old, new = a.runs
    elif len(a.runs) == 1:
        old, new = a.runs[0], saved[-1]
    elif len(saved) >= 2:
        old, new = saved[-2], saved[-1]
    else:
        sys.exit("need two runs in harness-out/runs/ (or pass the files)")
    skip = skipped()
    o = {r["id"]: r for r in load(old) if r["id"] not in skip}
    n = {r["id"]: r for r in load(new) if r["id"] not in skip}
    print(f"old: {os.path.basename(old)}\nnew: {os.path.basename(new)}\n")

    groups = {i: group(i) for i in o.keys() | n.keys()}
    print(f"{'':15} {'old':>9} {'new':>9}")
    for g in GROUPS:
        cnt = [(sum(r['verdict'] == 'PASS' for i, r in t.items() if groups[i] == g),
                sum(groups[i] == g for i in t)) for t in (o, n)]
        print(f"{g:15} {cnt[0][0]:3}/{cnt[0][1]:<5} {cnt[1][0]:3}/{cnt[1][1]:<5}")

    common = sorted(o.keys() & n.keys())
    flips = [i for i in common if o[i]["verdict"] != n[i]["verdict"]]
    if flips:
        print("\nverdict changes:")
        for i in flips:
            print(f"  {i:>11} {o[i]['verdict']:>5} -> {n[i]['verdict']:<5} [{groups[i]}] {n[i]['title'][:50]}")
    moves = []
    for i in common:
        a_, b_ = ssim(o[i]), ssim(n[i])
        if a_ is not None and b_ is not None and abs(b_ - a_) >= a.min:
            moves.append((b_ - a_, i, a_, b_))
    if moves:
        print(f"\nmean SSIM changes >= {a.min}:")
        for d, i, a_, b_ in sorted(moves):
            print(f"  {i:>11} {a_:.4f} -> {b_:.4f} ({d:+.4f}) [{groups[i]}] {n[i]['title'][:50]}")
    for label, ids in (("only in old", o.keys() - n.keys()), ("only in new", n.keys() - o.keys())):
        if ids:
            print(f"\n{label}: {' '.join(sorted(ids))}")
    if not (flips or moves):
        print("\nno verdict changes and no mean SSIM change above the threshold")
    if a.gate:
        bad = [i for i in flips if n[i]["verdict"] != "PASS"]
        bad += [i for d, i, _, _ in moves if d <= -a.drop and i not in bad]
        if bad:
            print(f"\nGATE FAILED: {len(bad)} regression(s): {' '.join(bad)}")
            sys.exit(1)
        print("\ngate: no regression")


if __name__ == "__main__":
    main()
