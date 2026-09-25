#!/usr/bin/env python3
"""Generate design-spec/references/tokens.css from design-tokens.json.

Reports, figures and screens paste tokens.css into `:root` instead of reading the
22 KB DTCG JSON on every render. The JSON stays the source of truth; the
builtins test `design_spec_tokens_css_matches_the_json` fails when the two
disagree, so rerun this after any token change:

    python3 scripts/gen-design-tokens-css.py

It writes the Claude copy and its Codex and Grok mirrors (byte-identical).
"""
import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent / "cas-cli/src/builtins"
REL = "skills/design-spec/references"
GENERIC_FAMILIES = {"serif", "sans-serif", "monospace", "cursive", "fantasy", "system-ui",
                    "ui-serif", "ui-sans-serif", "ui-monospace", "ui-rounded", "-apple-system"}


def family(names):
    return ", ".join(n if n in GENERIC_FAMILIES or " " not in n else f'"{n}"' for n in names)


def entries(group):
    return [(name, token) for name, token in group.items() if not name.startswith("$")]


def colors(mode, color):
    out = [(f"--{role}", token["$value"]) for role, token in entries(color[mode])]
    for group in ("series", "magnitude", "polarity"):
        for index, value in enumerate(color[group][mode]["$value"], 1):
            out.append((f"--{group}-{index}", value))
    out.append(("--series-neutral", color["series-neutral"][mode]["$value"]))
    return out


def root_block(tokens):
    typo = tokens["typography"]
    out = colors("light", tokens["color"])
    for name, token in entries(typo["family"]):
        out.append((f"--font-{name}", family(token["$value"])))
    for name, token in entries(typo["scale"]):
        value = token["$value"]
        out.append((f"--type-{name}",
                    f'{value["fontWeight"]} {value["fontSize"]}/{value["lineHeight"]} '
                    f'var(--font-{value["fontFamily"]})'))
    out.append(("--measure", typo["measure"]["$value"]))
    for step, token in entries(tokens["space"]):
        out.append((f"--space-{step}", token["$value"]))
    for name in ("container", "gutter", "margin-column"):
        out.append((f"--{name}", tokens["layout"][name]["$value"]))
    for name, token in entries(tokens["radius"]):
        out.append((f"--radius-{name}", token["$value"]))
    return out


def render(tokens):
    lines = [
        "/* Generated from design-tokens.json by scripts/gen-design-tokens-css.py. Do not edit.",
        "   Paste into the artifact's <style>; DESIGN.md tokens replace these when a project has one. */",
        ":root {",
        "  color-scheme: light dark;",
    ]
    lines += [f"  {name}: {value};" for name, value in root_block(tokens)]
    # Screen only: print is always light (html-reports technical contract).
    lines += ["}", "@media screen and (prefers-color-scheme: dark) {", "  :root {"]
    lines += [f"    {name}: {value};" for name, value in colors("dark", tokens["color"])]
    lines += ["  }", "}"]
    return "\n".join(lines) + "\n"


def main():
    source = ROOT / REL / "design-tokens.json"
    css = render(json.loads(source.read_text()))
    for flavour in ("", "codex/", "grok/"):
        target = ROOT / f"{flavour}{REL}" / "tokens.css"
        target.write_text(css)
        print(f"wrote {target.relative_to(ROOT.parent.parent.parent)}")


if __name__ == "__main__":
    main()
