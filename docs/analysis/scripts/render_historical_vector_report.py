#!/usr/bin/env python3
"""Render the cas-c505 Markdown source into one offline HTML report."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


STYLE = r"""
<style>
:root{color-scheme:light dark;--bg:#fbfbfd;--panel:#fff;--text:#17202a;--muted:#52606d;--line:#7b8794;--accent:#2457a7;--good:#176b3a;--warn:#8a4b08;--bad:#a22222;--soft:#eaf1fb}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:16px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}header,main,footer{max-width:1120px;margin:auto;padding:1.25rem}header{border-bottom:3px solid var(--accent)}h1{font-size:clamp(2rem,5vw,3.8rem);line-height:1.05;margin:.4rem 0}h2{margin-top:2.5rem;border-bottom:1px solid var(--line);padding-bottom:.35rem}h3{margin-top:1.7rem}.skip{position:absolute;left:-9999px}.skip:focus{left:1rem;top:1rem;background:var(--panel);padding:.6rem;z-index:10;outline:3px solid var(--accent)}a{color:var(--accent)}:focus-visible{outline:3px solid var(--accent);outline-offset:2px}.lead{font-size:1.25rem;max-width:78ch}.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:.8rem;margin:1rem 0}.card{background:var(--panel);border:1px solid var(--line);border-left:5px solid var(--accent);padding:1rem;border-radius:.35rem}.card strong{display:block;font-size:1.65rem;font-variant-numeric:tabular-nums}.table-wrap{overflow-x:auto;margin:1rem 0}table{border-collapse:collapse;width:100%;background:var(--panel);font-variant-numeric:tabular-nums}caption{text-align:left;font-weight:700;margin-bottom:.4rem}th,td{border:1px solid var(--line);padding:.55rem .65rem;text-align:left;vertical-align:top}thead th{background:var(--soft)}td.num,th.num{text-align:right}.total{font-weight:700;border-top:2px solid var(--text)}blockquote{border-left:5px solid var(--accent);margin:1rem 0;padding:.4rem 1rem;background:var(--panel)}pre{white-space:pre-wrap;overflow-wrap:anywhere;background:#111827;color:#f9fafb;padding:1rem;border-radius:.35rem}code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace}figure{margin:1.4rem 0;padding:1rem;border:1px solid var(--line);background:var(--panel);break-inside:avoid}figure svg{width:100%;height:auto}.chart-label{fill:var(--text);font:14px system-ui}.bar-kept{fill:var(--accent)}.bar-removed{fill:none;stroke:var(--warn);stroke-width:2}.zero{stroke:var(--line);stroke-width:1}figcaption,.provenance{color:var(--muted);font-size:.9rem}details{border:1px solid var(--line);padding:.65rem;margin:.7rem 0;background:var(--panel)}summary{font-weight:700;cursor:pointer}.status{font-weight:700}.status.open{color:var(--warn)}.status.covered{color:var(--good)}footer{border-top:1px solid var(--line);color:var(--muted)}
@media(prefers-color-scheme:dark){:root{--bg:#111827;--panel:#182235;--text:#f3f4f6;--muted:#c3cad5;--line:#8995a6;--accent:#3f83d6;--good:#71d99d;--warn:#d57420;--bad:#ff9696;--soft:#24334d}}
@media(max-width:520px){header,main,footer{padding:1rem}th,td{padding:.45rem;font-size:.9rem}.cards{grid-template-columns:1fr 1fr}.card strong{font-size:1.35rem}}
@media print{:root{--bg:#fff;--panel:#fff;--text:#000;--muted:#333;--line:#555;--accent:#174a8b;--soft:#eee}body{font-size:10.5pt}header,main,footer{max-width:none;padding:.4in}a[href^="http"]::after{content:" (" attr(href) ")";font-size:8pt}.table-wrap{overflow:visible}table{table-layout:fixed;font-size:7.5pt}th,td{overflow-wrap:anywhere;word-break:break-word;padding:.25rem}thead{display:table-header-group}tr,figure,.card,details{break-inside:avoid}details{display:block}details>*{display:block!important}.cards{grid-template-columns:repeat(4,1fr)}pre{background:#fff;color:#000;border:1px solid #555}}
</style>
"""


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    rendered = subprocess.run(
        ["pandoc", "--from=gfm+raw_html", "--to=html5", "--standalone",
         "--metadata", "pagetitle=Historical CAS operational vector index — 2026-08-17",
         str(args.source)], check=True, text=True, capture_output=True
    ).stdout
    rendered = rendered.replace("</head>", STYLE + "\n</head>")
    rendered = rendered.replace("<table>", '<div class="table-wrap"><table>')
    rendered = rendered.replace("</table>", "</table></div>")
    rendered = rendered.replace("<body>", '<body><a class="skip" href="#main">Skip to report</a><header><p>CAS operational intelligence · frozen evidence artifact</p></header><main id="main">', 1)
    rendered = rendered.replace("</body>", f'</main><footer><p>Markdown source: <a href="{args.source.name}">{args.source.name}</a>. Offline, dependency-free rendering.</p></footer></body>', 1)
    args.output.write_text(rendered)


if __name__ == "__main__":
    main()
