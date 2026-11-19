#!/usr/bin/env python3
"""Engine speed check: times `oce` on a synthetic tree (200k files) against `du` on the
same tree, so the numbers do not depend on how fast the machine is.

    bench.py OCE WORKDIR [--update]

Each ratio (oce time / du time, best of 3) must stay under 2x the one in baseline.json;
--update writes the measured ratios there instead. Only WORKDIR is written to.
"""

import json
import os
import subprocess
import sys
import time

DIRS, FILES = 2000, 100
BASELINE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "baseline.json")


def build(tree):
    marker = os.path.join(tree, ".complete")
    if os.path.exists(marker):
        return
    for d in range(DIRS):
        sub = os.path.join(tree, f"g{d % 20}", f"d{d}")
        os.makedirs(sub, exist_ok=True)
        for f in range(FILES):
            with open(os.path.join(sub, f"f{f}"), "wb") as fh:
                fh.write(b"x" * ((d * FILES + f) % 9000))
    open(marker, "w").close()


def best(cmd, env=None, fresh=None, runs=3):
    times = []
    for i in range(runs):
        if fresh:
            env = dict(os.environ, HOME=os.path.join(fresh, f"home-{time.time_ns()}"))
        start = time.perf_counter()
        subprocess.run(cmd, env=env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append(time.perf_counter() - start)
    return min(times)


def main():
    oce, work = sys.argv[1], sys.argv[2]
    # the same parallelism on every machine (du is single-threaded)
    os.environ["OC_THREADS"] = "2"
    tree = os.path.join(work, "tree")
    build(tree)
    du = best(["du", "-sk", tree])  # also warms the metadata cache
    du = min(du, best(["du", "-sk", tree]))
    # space: a fresh HOME per run is a cold directory cache; a shared one, after a first
    # run, is the repeat scan
    warm = dict(os.environ, HOME=os.path.join(work, "home-warm"))
    subprocess.run([oce, "space", tree], env=warm, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ratios = {
        "size": best([oce, "size", tree]) / du,
        "space": best([oce, "space", tree], fresh=work) / du,
        "space_repeat": best([oce, "space", tree], env=warm) / du,
    }
    print(f"du -sk: {du:.3f}s")
    for k, v in ratios.items():
        print(f"{k:>13}: {v:.2f}x du")
    if "--update" in sys.argv:
        with open(BASELINE, "w") as fh:
            json.dump({k: round(v, 2) for k, v in ratios.items()}, fh, indent=2)
            fh.write("\n")
        return 0
    with open(BASELINE) as fh:
        base = json.load(fh)
    failed = [k for k, v in ratios.items() if v > 2 * base[k]]
    for k in failed:
        print(f"::error::{k} is {ratios[k]:.2f}x du, more than twice the baseline {base[k]}x")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
