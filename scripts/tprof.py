#!/usr/bin/env python3
"""Summarise an Instruments Time Profiler trace without opening Instruments.

    scripts/tprof.py <trace> [N] [--thread SUBSTR] [--focus SUBSTR]

Prints sample share per thread, the top-N functions by self time and by
inclusive time (each counted once per sample), demangled where rustfilt or
c++filt is available. --thread keeps the samples of matching threads;
--focus keeps samples whose stack contains a matching function.
"""
import re, subprocess, sys, collections, xml.etree.ElementTree as ET

args = sys.argv[1:]
trace = args.pop(0)
n = int(args.pop(0)) if args and args[0].isdigit() else 25
opt = dict(zip(args[::2], args[1::2]))
xml = subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--xpath",
                      '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]'],
                     capture_output=True, text=True, check=True).stdout
root = ET.fromstring(xml)
ids = {}
def deref(e):
    if e is None:
        return None
    r = e.get("ref")
    if r is not None:
        return ids[r]
    if e.get("id") is not None:
        ids[e.get("id")] = e
    for c in e:
        deref(c)
    return e

def name_of(frame):
    return frame.get("name") or frame.get("addr") or "?"

threads = collections.Counter(); selfc = collections.Counter(); incl = collections.Counter(); total = 0
for row in root.iter("row"):
    for c in row:
        deref(c)
    th = row.find("thread"); th = ids.get(th.get("ref")) if th is not None and th.get("ref") else th
    tname = th.get("fmt", "?") if th is not None else "?"
    bt = row.find("tagged-backtrace")
    if bt is None:
        bt = row.find("backtrace")
    if bt is None:
        continue
    bt = ids.get(bt.get("ref")) if bt.get("ref") else bt
    frames = [ids.get(f.get("ref")) if f.get("ref") else f for f in bt.findall("frame")]
    names = [name_of(f) for f in frames if f is not None]
    if "--thread" in opt and opt["--thread"] not in tname:
        continue
    if "--focus" in opt and not any(opt["--focus"] in x for x in names):
        continue
    total += 1
    threads[re.sub(r"0x[0-9a-f]+", "", tname).strip()] += 1
    if names:
        selfc[names[0]] += 1
    for x in set(names):
        incl[x] += 1

def demangle(names):
    try:
        out = subprocess.run(["rustfilt"], input="\n".join(names), capture_output=True, text=True).stdout.splitlines()
        if len(out) == len(names):
            return out
    except FileNotFoundError:
        pass
    out = subprocess.run(["c++filt"], input="\n".join(names), capture_output=True, text=True).stdout.splitlines()
    return [re.sub(r"::h[0-9a-f]{16}$", "", x) for x in out] if len(out) == len(names) else names

print(f"{total} samples (1 ms each)")
print("\n-- threads")
for t, c in threads.most_common(15):
    print(f"{100 * c / total:5.1f} %  {t}")
for title, ctr in (("self", selfc), ("inclusive", incl)):
    print(f"\n-- {title}")
    top = ctr.most_common(n)
    for (x, c), d in zip(top, demangle([x for x, _ in top])):
        print(f"{100 * c / total:5.1f} %  {d[:150]}")
