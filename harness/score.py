#!/usr/bin/env python3
"""score.py [run.tsv]: fidelity score from harness-out/results.tsv (or a saved
run from harness-out/runs/), grouped by how much SceneScript a scene uses
(scripts run in the sandboxed worker, see docs/SCENESCRIPT.md).
Excluded scenes (harness/excluded.tsv) and captures waiting for a redo
(harness-out/recapture_after_unlock.txt) are left out."""
import csv, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "harness-out")
ROOTS = [os.path.expanduser("~/.local/share/Steam/steamapps/workshop/content/431960"),
         os.path.expanduser("~/.local/share/rustpaper-testset/steamapps/workshop/content/431960")]
GROUPS = ("no SceneScript", "1-9 scripts", "10+ scripts")


def load(src):
    return list(csv.DictReader(open(src), delimiter="\t"))


def skipped():
    skip = {l.split("\t")[0] for l in open(os.path.join(HERE, "excluded.tsv"))
            if l.strip() and not l.startswith("#")}
    redo = os.path.join(OUT, "recapture_after_unlock.txt")
    if os.path.exists(redo):
        skip |= set(open(redo).read().split())
    return skip


def scripts(i):
    for r in ROOTS:
        for f in ("scene.pkg", "scene.json", "gifscene.pkg"):
            p = os.path.join(r, i, f)
            if os.path.exists(p):
                return open(p, "rb").read().count(b'"script"')
    return 0


def group(i):
    n = scripts(i)
    return GROUPS[0] if n == 0 else (GROUPS[1] if n < 10 else GROUPS[2])


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(OUT, "results.tsv")
    skip = skipped()
    groups = {}
    for r in load(src):
        if r["id"] not in skip:
            groups.setdefault(group(r["id"]), []).append(r)
    total = ok = 0
    for g in GROUPS:
        v = groups.get(g, [])
        p = sum(r["verdict"] == "PASS" for r in v)
        ms = [float(r["mean_ssim"]) for r in v if r["mean_ssim"]]
        total += len(v); ok += p
        print(f"{g:15} {p:3}/{len(v):3} PASS ({100 * p / max(len(v), 1):3.0f}%)  "
              f"avg mean SSIM {sum(ms) / max(len(ms), 1):.3f}  mean SSIM >= 0.95: {sum(m >= 0.95 for m in ms)}")
    print(f"all            {ok:3}/{total:3} PASS ({100 * ok / max(total, 1):.0f}%)  [{len(skip)} excluded or awaiting recapture]")


if __name__ == "__main__":
    main()
