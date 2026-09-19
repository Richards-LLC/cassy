#!/usr/bin/env python3
"""Downscale committed PNG captures to at most MAX_W px wide and quantize to 256 colours (repository size)."""
import sys
from pathlib import Path
from PIL import Image
MAX_W = 1600
for arg in sys.argv[1:]:
    for f in sorted(Path(arg).glob('*.png')):
        im = Image.open(f); w, h = im.size
        if w > MAX_W:
            im = im.resize((MAX_W, round(h * MAX_W / w)), Image.LANCZOS)
        im = im.convert('RGB').quantize(colors=256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.FLOYDSTEINBERG)
        im.save(f, optimize=True)
        print(f.name, (w, h), '->', im.size)
