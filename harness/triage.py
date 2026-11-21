#!/usr/bin/env python3
"""triage.py [run.tsv]: why does each failing scene fail?

Puts every FAIL of harness-out/results.tsv (or a saved run) in ONE cause,
checked in this order:

  noise      the WE-vs-WE noise floor (we-floor.sh -> harness-out/floor.tsv)
             fails the same criterion: two runs of the real WE already
             disagree, so the metric cannot tell Rustpaper apart there
  missing    the renderer reported something unsupported, or a SceneScript
             failed (rp_warnings.txt)
  calibrate  everything is implemented and WE agrees with itself: the model
             of some feature is off

and counts, for each scene FEATURE (particle operators, effects, puppets,
audio, parallax...), how many PASS and FAIL scenes use it. A feature that
shows up in many "calibrate" failures and few passes is the next thing to
calibrate (in an isolated scene: calibration/isolate.sh, make_particles.py).
Correlation, not proof: use it to choose what to isolate.

Writes harness-out/triage.md and prints the summary."""
import csv, json, os, re, struct, sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from score import OUT, ROOTS, load, scripts, skipped  # noqa: E402

TEXT_EXT = (".json", ".frag", ".vert", ".h", ".txt")


def scene_files(i):
    """{name: bytes} for the JSON / shader files of a wallpaper (pkg or loose)."""
    for root in ROOTS:
        d = os.path.join(root, i)
        if not os.path.isdir(d):
            continue
        files = {}
        for pkg in ("scene.pkg", "gifscene.pkg"):
            p = os.path.join(d, pkg)
            if os.path.exists(p):
                with open(p, "rb") as f:
                    rd = lambda: f.read(struct.unpack("<I", f.read(4))[0]).decode("utf-8", "replace")
                    rd()
                    n = struct.unpack("<I", f.read(4))[0]
                    ents = [(rd(),) + struct.unpack("<II", f.read(8)) for _ in range(n)]
                    base = f.tell()
                    for name, off, ln in ents:
                        if name.endswith(TEXT_EXT):
                            f.seek(base + off)
                            files[name] = f.read(ln)
        for dp, _, fs in os.walk(d):
            for fn in fs:
                if fn.endswith(TEXT_EXT):
                    rel = os.path.relpath(os.path.join(dp, fn), d)
                    if rel not in files and os.path.getsize(os.path.join(dp, fn)) < 4 << 20:
                        files[rel] = open(os.path.join(dp, fn), "rb").read()
        return files
    return {}


def jload(b):
    try:
        return json.loads(b.decode("utf-8-sig", "replace"))
    except (ValueError, AttributeError):
        return None


def features(i):
    """Set of feature tags a scene uses."""
    files = scene_files(i)
    scene = None
    for k in ("scene.json", "gifscene.json"):
        if k in files:
            scene = jload(files[k])
    tags = set()
    if not isinstance(scene, dict):
        return tags
    g = scene.get("general", {})
    if g.get("cameraparallax"):
        tags.add("camera parallax")
    if g.get("bloom"):
        tags.add("bloom")
    if g.get("camerashake"):
        tags.add("camera shake")
    for o in scene.get("objects", []):
        if not isinstance(o, dict):
            continue
        if "particle" in o:
            tags.add("particles")
            p = jload(files.get(o["particle"], b"")) if isinstance(o["particle"], str) else None
            if isinstance(p, dict):
                for kind in ("operator", "initializer", "emitter", "renderer"):
                    for x in p.get(kind, []):
                        if isinstance(x, dict) and x.get("name"):
                            tags.add(f"particle {kind}: {x['name']}")
                if isinstance(p.get("flags"), int) and p["flags"] & 1:
                    tags.add("particles world-space")
                if p.get("controlpoint"):
                    tags.add("particle control points")
        if "text" in o:
            tags.add("text")
        if "sound" in o:
            tags.add("sound")
        if "light" in o:
            tags.add("light")
        if isinstance(o.get("image"), str):
            m = jload(files.get(o["image"], b""))
            if isinstance(m, dict) and m.get("puppet"):
                tags.add("puppet")
        if o.get("parent") is not None:
            tags.add("hierarchy")
        for e in o.get("effects", []) or []:
            f = e.get("file", "") if isinstance(e, dict) else ""
            if f.startswith("workshop/"):
                tags.add("effect: (workshop)")
            elif f:
                tags.add("effect: " + (f.split("/")[1] if "/" in f else f))
    blob = b"".join(files.values())
    if b"g_AudioSpectrum" in blob or b"audioprocessing" in blob:
        tags.add("audio-reactive")
    if b"g_PointerPosition" in blob:
        tags.add("pointer")
    if re.search(rb'"(alignment|orientation)"', blob):
        tags.add("text/sprite orientation")
    return tags


def failed(r):
    """Which pass criteria a row misses: {'mean', 'energy'}."""
    out = set()
    try:
        if float(r["mean_ssim"]) < 0.95:
            out.add("mean")
    except ValueError:
        out.add("mean")
    try:
        if not 0.7 <= float(r["energy_ratio"]) <= 1.4:
            out.add("energy")
    except ValueError:
        out.add("energy")
    return out


def warnings(i):
    """Renderer warnings and SceneScript errors (console output is not one)."""
    p = os.path.join(OUT, i, "rp_warnings.txt")
    if not os.path.exists(p):
        return []
    ws = set()
    for l in open(p, errors="replace"):
        if l.startswith("warning"):
            ws.add(re.sub(r"\d+", "N", l.strip()[9:90]))
        elif l.startswith("[script]") and re.search(r"Error|aborted|failed|stopped", l):
            ws.add(re.sub(r"\d+", "N", l.strip()[9:90]))
    return sorted(ws)


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(OUT, "results.tsv")
    skip = skipped()
    rows = [r for r in load(src) if r["id"] not in skip]
    fp = os.path.join(OUT, "floor.tsv")
    floor = {r["id"]: r for r in load(fp)} if os.path.exists(fp) else {}

    by_cause = defaultdict(list)
    feat = {}
    for r in rows:
        i = r["id"]
        feat[i] = features(i)
        if r["verdict"] == "PASS":
            continue
        miss = failed(r)
        fl = floor.get(i)
        why = ""
        if fl and miss <= failed(fl):
            cause = "noise"
            why = f"floor SSIM {float(fl['mean_ssim']):.3f}, ratio {fl['energy_ratio']}"
        elif warnings(i):
            cause, why = "missing", "; ".join(warnings(i))[:200]
        else:
            cause = "calibrate"
            if fl:
                why = f"floor SSIM {float(fl['mean_ssim']):.3f}, ratio {fl['energy_ratio']}"
        if scripts(i):
            why = f"{scripts(i)} scripts" + (f"; {why}" if why else "")
        by_cause[cause].append((r, miss, why))

    total = len(rows)
    npass = sum(r["verdict"] == "PASS" for r in rows)
    lines = [f"# Failure triage\n", f"{npass}/{total} PASS; floor measured for "
             f"{sum(r['id'] in floor for r in rows)} scenes.\n"]
    lines.append("| cause | scenes |\n|---|---|")
    for c in ("noise", "missing", "calibrate"):
        lines.append(f"| {c} | {len(by_cause[c])} |")
    for c in ("noise", "missing", "calibrate"):
        if not by_cause[c]:
            continue
        lines.append(f"\n## {c}\n\n| id | title | mean SSIM | energy | fails | detail |\n|---|---|---|---|---|---|")
        for r, miss, why in sorted(by_cause[c], key=lambda x: x[0]["mean_ssim"] or "0"):
            lines.append(f"| {r['id']} | {r['title'][:40]} | {r['mean_ssim'][:5]} | {r['energy_ratio']} | "
                         f"{'+'.join(sorted(miss))} | {why} |")

    # feature table: among the calibrate failures
    cal = {r["id"] for r, _, _ in by_cause["calibrate"]}
    passed = {r["id"] for r in rows if r["verdict"] == "PASS"}
    cnt_cal, cnt_pass = Counter(), Counter()
    for i in cal:
        cnt_cal.update(feat[i])
    for i in passed:
        cnt_pass.update(feat[i])
    lines.append("\n## Features of the `calibrate` failures\n\n"
                 "Scenes using each feature, among calibrate failures and passes. "
                 "High fail share + many failures = calibrate this next.\n\n"
                 "| feature | calibrate FAIL | PASS | fail share |\n|---|---|---|---|")
    ranked = sorted(cnt_cal, key=lambda t: (-cnt_cal[t] / (cnt_cal[t] + cnt_pass[t]), -cnt_cal[t]))
    for t in ranked:
        if cnt_cal[t] >= 2:
            share = cnt_cal[t] / (cnt_cal[t] + cnt_pass[t])
            lines.append(f"| {t} | {cnt_cal[t]} | {cnt_pass[t]} | {share:.0%} |")
    report = "\n".join(lines) + "\n"
    open(os.path.join(OUT, "triage.md"), "w").write(report)
    for c in ("noise", "missing", "calibrate"):
        print(f"{c:10} {len(by_cause[c]):3}")
    print(f"-> {os.path.join(OUT, 'triage.md')}")


if __name__ == "__main__":
    main()
