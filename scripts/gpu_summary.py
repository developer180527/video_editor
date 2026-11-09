#!/usr/bin/env python3
"""GPU time and command buffers per second from a Metal System Trace.

    scripts/gpu_summary.py <trace> [process-substring]

Prints GPU time by channel and encoder label (ms of GPU per second of
trace), the GPU's busy share (union of all intervals) and command buffers
submitted per second, by label.
"""
import collections, os, re, sys
sys.path.insert(0, os.path.dirname(__file__))
from xtable import rows

trace = sys.argv[1]
proc = sys.argv[2] if len(sys.argv) > 2 else "video-editor"

def us(s):
    v, u = s.split()
    return float(v.replace(",", "")) * {"ns": 1e-3, "µs": 1, "ms": 1e3, "s": 1e6}[u]

def t(s):
    m, sec = s.split(":")
    a, b, c = sec.split(".")
    return (int(m) * 60 + int(a)) * 1e6 + int(b) * 1e3 + int(c)

gi = [r for r in rows(trace, "metal-gpu-intervals") if proc in r["process"][0]]
starts = [t(r["start"][0]) for r in gi]
span = (max(starts) - min(starts)) / 1e6
busy = collections.Counter(); n = collections.Counter()
for r in gi:
    lab = re.sub(r"\s+\(" + re.escape(proc) + r".*", "", r["event-label"][0])
    lab = re.sub(r"\d+", "#", lab)
    k = (r["channel-name"][0], lab)
    busy[k] += us(r["duration"][0]); n[k] += 1
print(f"span {span:.1f} s")
for k, v in busy.most_common(12):
    print(f"{v / span / 1e3:7.2f} ms/s  {n[k] / span:6.1f}/s  {k[0]:9} {k[1]}")
iv = sorted((t(r["start"][0]), t(r["start"][0]) + us(r["duration"][0])) for r in gi)
tot = 0; cs, ce = iv[0]
for s, e in iv[1:]:
    if s > ce:
        tot += ce - cs; cs, ce = s, e
    else:
        ce = max(ce, e)
tot += ce - cs
print(f"GPU busy {tot / span / 1e3:.2f} ms/s ({tot / span / 1e4:.2f} %)")
cbs = collections.defaultdict(set)
for r in rows(trace, "metal-application-encoders-list"):
    if proc in r["process"][0]:
        cbs[re.sub(r"\d+", "#", r["cmdbuffer-label"][0])].add(r["cmdbuffer-id"][0])
total = sum(len(v) for v in cbs.values())
print(f"command buffers {total / span:.1f}/s: " + ", ".join(f"{k} {len(v) / span:.1f}" for k, v in sorted(cbs.items(), key=lambda kv: -len(kv[1]))))
