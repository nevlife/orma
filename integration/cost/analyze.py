#!/usr/bin/env python3
"""Summarize a campaign (design 8.6): latency tail and resources per workload, rate, sched,
topics and method.

usage: analyze.py OUT_DIR

Latency is publisher stamp to sink callback entry, samples whose arrival falls in the measured
window, pooled over reps. `>none p99` is the share of samples above the pooled p99 of the
`none` method of the same block (1 % for `none` by construction); `>1ms` is an absolute
threshold. CPU % is (utime+stime) over the window, 100 = one core.
"""

import argparse
import collections
import glob
import json
import os

import numpy as np


def load_run(d):
    m = json.load(open(os.path.join(d, "meta.json")))
    if "pending" in m:
        return m, None
    lats, lost, got = [], 0, 0
    for f in glob.glob(os.path.join(d, "*_t*.csv")):
        a = np.loadtxt(f, delimiter=",", skiprows=1, dtype=np.int64, ndmin=2)
        a = a[(a[:, 2] >= m["t0"]) & (a[:, 2] < m["t1"])]
        lats.append((a[:, 2] - a[:, 1]) / 1e3)
        seqs = np.unique(a[:, 0])
        lost += int(seqs[-1] - seqs[0] + 1 - len(seqs))
        got += len(a)
    m["n"] = got
    m["lost"] = lost
    return m, np.concatenate(lats)


def resources(m):
    win = (m["t1"] - m["t0"]) / 1e9
    clk = m["clk_tck"]
    out = collections.defaultdict(float)
    for key, s0 in m["proc0"].items():
        s1 = m["proc1"][key]
        label = key.rsplit(":", 1)[0]
        cpu = (s1["utime"] + s1["stime"] - s0["utime"] - s0["stime"]) / clk / win * 100
        if label.startswith("w:"):
            out["cpu_workload"] += cpu
            out["csw_per_s"] += (s1["vcsw"] + s1["ivcsw"] - s0["vcsw"] - s0["ivcsw"]) / win
            out["ivcsw_per_s"] += (s1["ivcsw"] - s0["ivcsw"]) / win
            out[f"cpu_{label[2:]}"] = cpu
        else:
            out["cpu_" + label.split("-")[-1]] += cpu
    out["pss_workload_kb"] = sum(v.get("pss", 0) for v in m["mem"].values())
    out["rss_workload_kb"] = sum(v.get("rss", 0) for v in m["mem"].values())
    if m.get("mem_daemons"):
        out["pss_daemons_kb"] = sum(v.get("pss", 0) for v in m["mem_daemons"].values())
    up0, up1 = m["method0"].get("uprobe"), m["method1"].get("uprobe")
    if up0 and up1:
        dt = up1["bpf_run_time_ns"] - up0["bpf_run_time_ns"]
        dc = up1["bpf_run_cnt"] - up0["bpf_run_cnt"]
        out["bpf_cpu_pct"] = dt / 1e9 / win * 100
        out["bpf_runs_per_s"] = dc / win
        out["bpf_ns_per_run"] = dt / dc if dc else 0
    for st in m["method_stats"].values():
        for k, v in st.items():
            out[k] += v
    return dict(out)


def q(x, p):
    return float(np.percentile(x, p))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    a = ap.parse_args()
    groups = collections.defaultdict(list)
    pending = {}
    for d in sorted(glob.glob(os.path.join(a.out, "r*_*"))):
        if not os.path.exists(os.path.join(d, "meta.json")):
            continue
        m, lat = load_run(d)
        key = (m["workload"], m["rate"], m["sched"], m["topics"], m["method"])
        if lat is None:
            pending[key] = m["pending"]
            continue
        groups[key].append((m, lat))

    rows = []
    blocks = sorted({k[:4] for k in groups})
    for blk in blocks:
        base = np.concatenate([l for _, l in groups[blk + ("none",)]])
        thr, thr9 = q(base, 99), q(base, 99.9)
        for key in sorted(k for k in groups if k[:4] == blk):
            runs = groups[key]
            lat = np.concatenate([l for _, l in runs])
            res = [resources(m) for m, _ in runs]
            keys = sorted({k for r in res for k in r})
            rmean = {k: float(np.mean([r.get(k, 0.0) for r in res])) for k in keys}
            win = sum((m["t1"] - m["t0"]) / 1e9 for m, _ in runs)
            row = {
                "workload": key[0],
                "rate": key[1],
                "sched": key[2],
                "topics": key[3],
                "method": key[4],
                "reps": len(runs),
                "n": int(len(lat)),
                "lost": sum(m["lost"] for m, _ in runs),
                "p50": q(lat, 50),
                "p90": q(lat, 90),
                "p99": q(lat, 99),
                "p999": q(lat, 99.9),
                "max": float(lat.max()),
                "p999_reps": [q(l, 99.9) for _, l in runs],
                "max_reps": [float(l.max()) for _, l in runs],
                "gt_none_p99_pct": float((lat > thr).mean() * 100),
                "gt_none_p999_pct": float((lat > thr9).mean() * 100),
                "gt_1ms_pct": float((lat > 1000).mean() * 100),
                "none_p99": thr,
                "rate_per_s": len(lat) / win,
                **rmean,
            }
            rows.append(row)
    for key, why in sorted(pending.items()):
        rows.append(
            {
                "workload": key[0],
                "rate": key[1],
                "sched": key[2],
                "topics": key[3],
                "method": key[4],
                "pending": why,
            }
        )

    cur = None
    for r in rows:
        blk = (r["workload"], r["rate"], r["sched"], r["topics"])
        if blk != cur:
            cur = blk
            print(f"\n== {blk[0]} {blk[1]} Hz {blk[2]} topics {blk[3]}")
            print(
                f"{'method':24} {'p50':>7} {'p90':>7} {'p99':>7} {'p99.9':>8} {'max':>8} "
                f"{'>n99%':>6} {'>n999%':>6} {'>1ms%':>6} {'lost':>5} {'cpuW':>6} "
                f"{'armod':>6} {'bpf%':>6} {'lttng':>6} {'csw/s':>7} {'pssMB':>6}  p99.9 reps"
            )
        if "pending" in r:
            print(f"{r['method']:24} pending: {r['pending']}")
            continue
        sess = r.get("cpu_sessiond", 0) + r.get("cpu_consumerd", 0)
        print(
            f"{r['method']:24} {r['p50']:7.1f} {r['p90']:7.1f} {r['p99']:7.1f} "
            f"{r['p999']:8.1f} {r['max']:8.1f} {r['gt_none_p99_pct']:6.2f} "
            f"{r['gt_none_p999_pct']:6.3f} {r['gt_1ms_pct']:6.3f} {r['lost']:5d} "
            f"{r.get('cpu_workload', 0):6.1f} {r.get('cpu_armod', 0):6.2f} "
            f"{r.get('bpf_cpu_pct', 0):6.2f} {sess:6.2f} {r.get('csw_per_s', 0):7.0f} "
            f"{r.get('pss_workload_kb', 0) / 1024:6.1f}  "
            + " ".join(f"{x:.0f}" for x in r["p999_reps"])
        )


if __name__ == "__main__":
    main()
