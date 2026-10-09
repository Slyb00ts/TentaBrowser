"""Reads the probe points of a parity page out of a Chrome screenshot and prints
the Rust `CHROME` table of crates/tenta-browser/tests/images.rs.

Usage: probe_table.py DIR   (reads DIR/chrome.png and DIR/probes.json)
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from chrome_shot import read_png

directory = sys.argv[1]
width, height, bpp, rows = read_png(os.path.join(directory, "chrome.png"))
probes = json.load(open(os.path.join(directory, "probes.json")))
print("/// Colours Chrome 154 drew at the probe points of each cell of the parity page")
print("/// (`probes`, row-major), as `[r, g, b]` over the white page.")
print(f"const CHROME: [[[u8; 3]; 16]; {len(probes)}] = [")
for cell in probes:
    points = ", ".join("[%d, %d, %d]" % tuple(rows[y][x * bpp:x * bpp + 3]) for x, y in cell)
    print(f"    [{points}],")
print("];")
