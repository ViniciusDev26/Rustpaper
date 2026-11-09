#!/usr/bin/env python3
"""score.py: fidelity score from harness-out/results.tsv, grouped by how much
SceneScript a scene uses (scripts are never executed, so heavily scripted
scenes cannot match). Excluded scenes (harness/excluded.tsv) and captures
waiting for a redo (harness-out/recapture_after_unlock.txt) are left out."""
import csv, os
here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, "..", "harness-out")
rows = list(csv.DictReader(open(os.path.join(out, "results.tsv")), delimiter="\t"))
roots = [os.path.expanduser("~/.local/share/Steam/steamapps/workshop/content/431960"),
         os.path.expanduser("~/.local/share/rustpaper-testset/steamapps/workshop/content/431960")]
skip = {l.split("\t")[0] for l in open(os.path.join(here, "excluded.tsv")) if l.strip() and not l.startswith("#")}
redo = os.path.join(out, "recapture_after_unlock.txt")
if os.path.exists(redo):
    skip |= set(open(redo).read().split())
def scripts(i):
    for r in roots:
        for f in ("scene.pkg", "scene.json", "gifscene.pkg"):
            p = os.path.join(r, i, f)
            if os.path.exists(p):
                return open(p, "rb").read().count(b'"script"')
    return 0
groups = {}
for r in rows:
    if r["id"] in skip:
        continue
    n = scripts(r["id"])
    g = "no SceneScript" if n == 0 else ("1-9 scripts" if n < 10 else "10+ scripts")
    groups.setdefault(g, []).append(r)
total = ok = 0
for g in ("no SceneScript", "1-9 scripts", "10+ scripts"):
    v = groups.get(g, [])
    p = sum(r["verdict"] == "PASS" for r in v)
    ms = [float(r["mean_ssim"]) for r in v if r["mean_ssim"]]
    total += len(v); ok += p
    print(f"{g:15} {p:3}/{len(v):3} PASS ({100 * p / max(len(v), 1):3.0f}%)  "
          f"avg mean SSIM {sum(ms) / max(len(ms), 1):.3f}  mean SSIM >= 0.95: {sum(m >= 0.95 for m in ms)}")
print(f"all            {ok:3}/{total:3} PASS ({100 * ok / max(total, 1):.0f}%)  [{len(skip)} excluded or awaiting recapture]")
