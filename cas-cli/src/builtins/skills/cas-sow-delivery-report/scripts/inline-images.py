#!/usr/bin/env python3
"""Make a report HTML single-file by inlining its local images.

Usage: python3 inline-images.py <draft.html> <final.html> [--max-image-kb 250] [--max-total-mb 8]

Every <img src="relative/path.(jpg|jpeg|png|webp|svg)"> and CSS url(relative/path)
becomes a data URI. Remote URLs are refused, because the report must render offline.
The budget flags report oversize images and the total; the exit code is 1 when a
budget or a reference fails, and the output is still written so you can inspect it.
"""
import base64
import mimetypes
import re
import sys
from pathlib import Path

MIME = {".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".png": "image/png",
        ".webp": "image/webp", ".svg": "image/svg+xml", ".woff2": "font/woff2", ".woff": "font/woff"}


def main(argv):
    pos = [a for i, a in enumerate(argv) if not a.startswith("--") and not (i and argv[i - 1].startswith("--"))]
    if len(pos) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    opt = {argv[i][2:]: argv[i + 1] for i in range(len(argv) - 1) if argv[i].startswith("--")}
    max_img = float(opt.get("max-image-kb", 250)) * 1024
    max_total = float(opt.get("max-total-mb", 8)) * 1024 * 1024
    src, dst = Path(pos[0]).resolve(), Path(pos[1]).resolve()
    html = src.read_text(encoding="utf-8")
    problems, sizes = [], {}

    def encode(ref):
        if re.match(r"^(https?:)?//", ref):
            problems.append(f"remote reference refused: {ref}")
            return ref
        if ref.startswith("data:") or ref.startswith("#"):
            return ref
        path = (src.parent / ref).resolve()
        if not path.is_file():
            problems.append(f"missing file: {ref}")
            return ref
        data = path.read_bytes()
        sizes[ref] = len(data)
        if path.suffix.lower() in (".jpg", ".jpeg", ".png", ".webp") and len(data) > max_img:
            problems.append(f"image over {max_img / 1024:.0f} KB: {ref} ({len(data) / 1024:.0f} KB); crop it to the region that proves the row")
        mime = MIME.get(path.suffix.lower()) or mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        return f"data:{mime};base64,{base64.b64encode(data).decode('ascii')}"

    html = re.sub(r'(<img\b[^>]*?\bsrc=)(["\'])(.*?)\2', lambda m: f"{m.group(1)}{m.group(2)}{encode(m.group(3))}{m.group(2)}", html)
    html = re.sub(r'url\((["\']?)(?!data:)([^)"\']+)\1\)', lambda m: f"url({m.group(1)}{encode(m.group(2))}{m.group(1)})", html)
    dst.write_text(html, encoding="utf-8")
    total = dst.stat().st_size
    if total > max_total:
        problems.append(f"file is {total / 1048576:.1f} MB, over the {max_total / 1048576:.0f} MB budget")
    print(f"inlined {len(sizes)} file(s), {sum(sizes.values()) / 1024:.0f} KB raw → {dst} ({total / 1024:.0f} KB)")
    for p in problems:
        print(f"FAIL {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
