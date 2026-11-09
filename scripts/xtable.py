#!/usr/bin/env python3
"""Rows of one table of an Instruments trace, as dicts of formatted values.

    from xtable import rows; for r in rows(trace, "metal-gpu-intervals"): ...
    scripts/xtable.py <trace> <schema> [N]      print the first N rows

Each value is (fmt, raw): what Instruments shows, and the number under it.
"""
import subprocess, sys, xml.etree.ElementTree as ET

def rows(trace, schema):
    xml = subprocess.run(["xcrun", "xctrace", "export", "--input", trace, "--xpath",
                          f'/trace-toc/run[@number="1"]/data/table[@schema="{schema}"]'],
                         capture_output=True, text=True, check=True).stdout
    root = ET.fromstring(xml)
    cols = [m.text for m in root.iter("mnemonic")]
    ids = {}
    def walk(e):
        if e.get("id") is not None:
            ids[e.get("id")] = e
        for c in e:
            walk(c)
    for row in root.iter("row"):
        walk(row)
        out = {}
        for name, c in zip(cols, list(row)):
            if c.get("ref") is not None:
                c = ids[c.get("ref")]
            out[name] = (c.get("fmt") or (c.text or ""), c.text)
        yield out

if __name__ == "__main__":
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    for i, r in enumerate(rows(sys.argv[1], sys.argv[2])):
        if i >= n:
            break
        print({k: v[0] for k, v in r.items()})
