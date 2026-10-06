#!/usr/bin/env python3
"""Map changed paths to the user journeys they touch (docs/qa/journeys.md).

Usage:
  journeys-for-diff.py <base-ref> [<head-ref>]   paths changed base..head (default HEAD)
  journeys-for-diff.py --paths <path>...         explicit repo-relative paths
  journeys-for-diff.py --all                     every catalog journey
  journeys-for-diff.py --check                   validate the catalog; exit 1 on errors

Prints JSON: {"catalog": ..., "journeys": [{"id", "title", "surface", "suite", "reason"}]}.
Derived hub-web/dist/** is ignored. Browser source roots are declared by
catalog Touches and expanded through static relative imports from source and
specs. main.ts and shared fixture hunks use symbol ownership; styles.css uses
changed selectors. Global tokens/base rules, runtime/build configuration and
unattributed changes select the surface with an explanatory reason.

Diff mode retains base...head semantics and reads the catalog/graph at head.
For --paths/--check/--all from another checkout, set CAS_JOURNEYS_HEAD to the
reviewed revision; CAS_JOURNEYS_BASE supplies CSS/main/fixture hunk context.
Without that context those files select the surface safely. Uncertain source
parsing selects every catalog journey with a reason; Git/catalog errors exit
nonzero. An unrelated/derived-only diff produces an empty list and exit 0.

The catalog contract is described in docs/qa/journey-evaluation.md.
"""

from __future__ import annotations

import fnmatch
import json
import os
import re
import subprocess
import sys
from pathlib import Path

CATALOG = "docs/qa/journeys.md"
REQUIRED = ("Entry", "Goal", "Touches", "Suite", "Gaps")
BLOCKS = ("Steps", "Expected experience", "Edge paths")
ID_RE = re.compile(r"^[A-Z]+-J[0-9]+$")


def repo_root() -> Path:
    override = os.environ.get("CAS_JOURNEYS_ROOT")
    if override:
        return Path(override)
    out = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    return Path(out.stdout.strip())


def globs(value: str) -> list[str]:
    return re.findall(r"`([^`]+)`", value)


def parse(text: str) -> tuple[dict[str, list[str]], list[dict]]:
    """Return ({surface: surface-wide globs}, [journey])."""
    surfaces: dict[str, list[str]] = {}
    journeys: list[dict] = []
    surface = None
    current = None
    block = None
    for line in text.splitlines():
        if line.startswith("## "):
            surface = line[3:].strip()
            surfaces.setdefault(surface, [])
            current, block = None, None
            continue
        if line.startswith("### "):
            head = line[4:].strip()
            ident, _, title = head.partition(" · ")
            current = {"id": ident.strip(), "title": title.strip(), "surface": surface, "fields": {}, "blocks": {}}
            journeys.append(current)
            block = None
            continue
        field = re.match(r"^- \*\*([A-Za-z -]+):\*\*\s*(.*)$", line)
        if field and current is None and surface is not None:
            if field.group(1) == "Surface-wide":
                surfaces[surface] = globs(field.group(2))
            continue
        if current is None:
            continue
        if field and block is None:
            current["fields"][field.group(1)] = field.group(2).strip()
            continue
        heading = re.match(r"^(?:\*\*([A-Za-z ]+)\*\*|#### ([A-Za-z ]+))\s*$", line)
        if heading:
            block = heading.group(1) or heading.group(2)
            current["blocks"][block] = []
            continue
        item = re.match(r"^\s*(?:[0-9]+\.|-)\s+(.*)$", line)
        if item and block is not None:
            current["blocks"][block].append(item.group(1).strip())
    return surfaces, journeys


def suite_path(journey: dict) -> str | None:
    found = globs(journey["fields"].get("Suite", ""))
    return found[0] if found else None


def check(root: Path, surfaces: dict[str, list[str]], journeys: list[dict], tree=None) -> list[str]:
    errors: list[str] = []
    if not journeys:
        errors.append("catalog has no journeys")
    seen: set[str] = set()
    for surface, wide in surfaces.items():
        if not wide:
            errors.append(f"surface {surface}: missing **Surface-wide:** globs")
    for journey in journeys:
        ident = journey["id"]
        where = f"{ident or '<no id>'}"
        if not ID_RE.match(ident):
            errors.append(f"{where}: id must look like HUB-J1")
        if ident in seen:
            errors.append(f"{where}: duplicate id")
        seen.add(ident)
        if not journey["title"]:
            errors.append(f"{where}: heading must be '### <ID> · <title>'")
        for name in REQUIRED:
            if not journey["fields"].get(name):
                errors.append(f"{where}: missing **{name}:**")
        if not globs(journey["fields"].get("Touches", "")):
            errors.append(f"{where}: **Touches:** needs at least one `glob`")
        for name in BLOCKS:
            if not journey["blocks"].get(name):
                errors.append(f"{where}: missing non-empty **{name}** list")
        suite = journey["fields"].get("Suite", "")
        spec = suite_path(journey)
        if spec is None:
            if not suite.startswith("not automated"):
                errors.append(f"{where}: **Suite:** must be a `spec path` or 'not automated — <reason>'")
            continue
        spec_file = root / spec
        exists = spec in tree.paths if tree else spec_file.is_file()
        if not exists:
            errors.append(f"{where}: suite {spec} does not exist")
            continue
        body = tree.read(spec) if tree else spec_file.read_text()
        if not re.search(rf"""['"`]{re.escape(ident)}\b""", body):
            errors.append(f"{where}: {spec} has no test titled with {ident}")
        for step in journey["blocks"].get("Steps", []):
            title = step.split(" — ")[0].strip()
            if title and title not in body:
                errors.append(f"{where}: step '{title}' has no matching test.step in {spec}")
    return errors


def changed_paths(root: Path, base: str, head: str) -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(root), "diff", "--name-only", f"{base}...{head}"],
        capture_output=True, text=True,
    )
    if out.returncode != 0:
        sys.stderr.write(out.stderr)
        raise SystemExit(2)
    return [line for line in out.stdout.splitlines() if line]


class SourceTree:
    """Read a reviewed revision without checking it out, or the working tree."""
    def __init__(self, root: Path, revision: str | None = None):
        self.root, self.revision = root, revision
        self.cache: dict[str, str] = {}
        if revision:
            result = subprocess.run(["git", "-C", str(root), "ls-tree", "-r", "--name-only", revision], capture_output=True, text=True, check=True)
            self.paths = set(result.stdout.splitlines())
        else:
            self.paths = {str(p.relative_to(root)) for folder in ("hub-web/src", "hub-web/e2e", "hub-web/scripts") for p in (root / folder).rglob("*") if p.is_file()}
            self.paths.update(p for p in (CATALOG, "hub-web/package.json", "hub-web/package-lock.json", "hub-web/vite.config.ts", "hub-web/index.html") if (root / p).is_file())

    def read(self, path: str) -> str:
        if path not in self.cache:
            if path not in self.paths:
                self.cache[path] = ""
            elif self.revision:
                self.cache[path] = subprocess.run(["git", "-C", str(self.root), "show", f"{self.revision}:{path}"], capture_output=True, text=True, check=True).stdout
            else:
                self.cache[path] = (self.root / path).read_text()
        return self.cache[path]

    def resolve(self, importer: str, request: str) -> str | None:
        if not request.startswith("."):
            return None
        path = os.path.normpath(str(Path(importer).parent / request))
        candidates = [path, *(path + ext for ext in (".ts", ".tsx", ".js", ".mjs")), *(path + "/index" + ext for ext in (".ts", ".tsx", ".js", ".mjs"))]
        if path.endswith((".js", ".mjs")):
            candidates.extend([str(Path(path).with_suffix(".ts")), str(Path(path).with_suffix(".tsx"))])
        return next((p for p in candidates if p in self.paths), None)


# These are source import statements, not strings passed to page.goto. The
# catalog Touches fields provide the explicit bridge to browser-owned modules.
IMPORT_RE = re.compile(r"\b(?:import|export)\s+(?:[^;]*?\s+from\s*)?[\"'](\.[^\"']+)[\"']", re.MULTILINE)
def mask_comments(text: str, strings: bool = False) -> str:
    """Mask lexical literals without mistaking regex quotes/braces for code.

    Preserve offsets/newlines for hunk ownership. A slash after an operand is
    division; expression-start punctuation/keywords can introduce a regex.
    This remains a conservative source scanner, not a TypeScript parser.
    """
    output = list(text)
    previous = ""
    index = 0
    word_token = re.compile(r"[\w$]+")

    def mask(start: int, end: int):
        for pos in range(start, end):
            if output[pos] != "\n":
                output[pos] = " "

    while index < len(text):
        char = text[index]
        if char.isspace():
            index += 1
            continue
        start = index
        if text.startswith("//", index) or text.startswith("/*", index):
            if text.startswith("//", index):
                end = text.find("\n", index)
                index = len(text) if end < 0 else end
            else:
                end = text.find("*/", index + 2)
                index = len(text) if end < 0 else end + 2
            mask(start, index)
            continue
        if char in "\"'`":
            index += 1
            while index < len(text):
                if text[index] == "\\":
                    index += 2
                elif text[index] == char:
                    index += 1
                    break
                else:
                    index += 1
            index = min(index, len(text))
            if strings:
                mask(start, index)
            previous = "literal"
            continue
        if char == "/" and (previous in ("", "(", "[", "{", "=", ":", ",", ";", "!", "?", "&", "|", ">")
                            or previous in ("return", "throw", "case", "yield", "void", "typeof", "delete", "await")):
            index += 1
            in_class = False
            while index < len(text) and text[index] != "\n":
                if text[index] == "\\":
                    index += 2
                    continue
                if text[index] == "[":
                    in_class = True
                elif text[index] == "]":
                    in_class = False
                elif text[index] == "/" and not in_class:
                    index += 1
                    while index < len(text) and text[index].isalpha():
                        index += 1
                    break
                index += 1
            else:
                raise ValueError("Unterminated regex literal")
            mask(start, index)
            previous = "literal"
            continue
        word = word_token.match(text, index)
        if word:
            previous = word.group()
            index += len(previous)
        else:
            previous = char
            index += 1
    return "".join(output)


def imported_modules(tree: SourceTree, path: str) -> set[str]:
    if not path.endswith((".ts", ".tsx", ".js", ".mjs")):
        return set()
    text = mask_comments(tree.read(path))
    requests = IMPORT_RE.findall(text) + re.findall(r"\bimport\s*\(\s*[\"'](\.[^\"']+)[\"']\s*\)", text)
    return {found for request in requests if (found := tree.resolve(path, request)) and not found.startswith("hub-web/dist/")}


def closure(tree: SourceTree, roots: set[str]) -> set[str]:
    visited: set[str] = set()
    pending = list(roots)
    while pending:
        path = pending.pop()
        if path in visited:
            continue
        visited.add(path)
        pending.extend(imported_modules(tree, path) - visited)
    return visited


def css_rules(text: str) -> dict[tuple[tuple[str, ...], str], str]:
    """Retain conditional context; a changed media query changes its children."""
    text = mask_comments(text)
    rules: dict[tuple[tuple[str, ...], str], str] = {}
    def parse_block(start: int, end: int, context: tuple[str, ...]):
        cursor = start
        while cursor < end:
            opening = text.find("{", cursor, end)
            semicolon = text.find(";", cursor, end)
            if semicolon >= 0 and (opening < 0 or semicolon < opening):
                prelude = text[cursor:semicolon].strip()
                if prelude:
                    rules[(context, prelude)] = ";"
                cursor = semicolon + 1
                continue
            if opening < 0:
                if text[cursor:end].strip():
                    raise ValueError("Unparsed CSS trailing text")
                break
            prelude = re.sub(r"\s+", " ", text[cursor:opening].strip())
            closing = matching_brace(mask_comments(text, strings=True), opening)
            if closing >= end:
                raise ValueError("Unbalanced CSS rule")
            if re.match(r"@(media|supports|container|layer|scope)\b", prelude):
                parse_block(opening + 1, closing, context + (prelude,))
            else:
                key = (context, prelude)
                # Duplicate selectors cascade: keep each occurrence's body.
                rules[key] = rules.get(key, "") + re.sub(r"\s+", " ", text[opening + 1:closing].strip()) + "\n"
            cursor = closing + 1
    parse_block(0, len(text), ())
    return rules


def matching_brace(text: str, opening: int) -> int:
    depth = 0
    for index in range(opening, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise ValueError("Unbalanced brace")


def global_selector(selector: str) -> bool:
    return selector.startswith("@") or any(":root" in part or not re.search(r"[.#][\w-]+", part) for part in selector.split(","))


def selector_anchors(selector: str) -> set[str]:
    # Use the final compound selector, not common ancestor .conversation-context,
    # so one rail component does not select every page using the shell.
    anchors: set[str] = set()
    for part in selector.split(","):
        compound = re.split(r"\s+|[>+~]", part.strip())[-1]
        anchors.update(re.findall(r"[.#]([\w-]+)", compound))
    return anchors


def additive_recorders(before: str, after: str, old_region: str, new_region: str) -> set[str]:
    """Allow only new observation fields in an otherwise byte-preserved dispatch.

    Unknown statement shapes stay wide. In particular, existing branch/method
    lines cannot change, and arbitrary recorder calls are not assumed pure.
    """
    import difflib
    added = []
    for tag, _, _, start, end in difflib.SequenceMatcher(
            None, old_region.splitlines(), new_region.splitlines(), autojunk=False).get_opcodes():
        if tag not in ("equal", "insert"):
            return set()
        if tag == "insert":
            added.extend(new_region.splitlines()[start:end])
    names: set[str] = set()
    for line in added:
        code = mask_comments(line).strip()
        if not code:
            continue
        match = re.fullmatch(
            r"(?:if\s*\(\s*([\w$]+)\.([\w$]+)\s*\)\s*)?"
            r"this\.([\w$]+)(?:\.push\((.*)\)|\s*=\s*(.*));", code)
        if not match:
            return set()
        receiver, guard, field, pushed, assigned = match.groups()
        if re.search(rf"\b{re.escape(field)}\b", mask_comments(before)):
            return set()
        if not re.search(rf"\b(?:readonly|private|public|protected)\s+{re.escape(field)}\s*[:=]", mask_comments(after)):
            return set()
        if guard and re.search(rf"\b{re.escape(receiver)}\s*\.\s*{re.escape(guard)}\b", mask_comments(old_region)):
            return set()
        raw_value = pushed if pushed is not None else assigned
        if "`" in raw_value:
            return set()
        value = mask_comments(raw_value, strings=True)
        if re.search(r"\b(?:await|return|throw|new|delete|yield|if|else)\b|[;=?!]|\+\+|--|`", value):
            return set()
        if any(call not in ("Number", "String", "Boolean") for call in re.findall(r"([\w$.]+)\s*\(", value)):
            return set()
        # Reject property accessors, spread, computed calls and other syntax
        # outside inert record literals/properties plus primitive conversions.
        if re.search(r"\.\.\.|[\[\]]|\b(?:get|set)\s+[\w$]+\s*\(", value):
            return set()
        names.add(field)
    return names


class MainModel:
    def __init__(self, tree: SourceTree, path: str):
        self.text = tree.read(path)
        self.clean = mask_comments(self.text, strings=True)
        self.imports: list[tuple[int, int, str]] = []
        self.bindings: dict[str, str] = {}
        for match in re.finditer(r"\bimport\s+([^;]+?)\s+from\s*[\"'](\.[^\"']+)[\"']", mask_comments(self.text), re.DOTALL):
            module = tree.resolve(path, match.group(2))
            if not module:
                continue
            self.imports.append((match.start(), match.end(), module))
            clause = match.group(1).strip()
            if clause.startswith("type "):
                continue
            named = re.search(r"\{(.*?)\}", clause, re.DOTALL)
            if named:
                for item in named.group(1).split(","):
                    item = item.strip()
                    if item and not item.startswith("type "):
                        self.bindings[item.split(" as ")[-1].strip()] = module
                clause = clause[:named.start()].rstrip(" ,")
            namespace = re.search(r"\*\s+as\s+(\w+)", clause)
            if namespace:
                self.bindings[namespace.group(1)] = module
            elif re.fullmatch(r"[\w$]+", clause):
                self.bindings[clause] = module
        # Preserve ownership through a composition-local instance/store alias.
        for match in re.finditer(r"\b(?:const|let)\s+(\w+)[^=;]*=\s*(?:new\s+)?(\w+)\s*\(", self.clean):
            prefix = self.clean[:match.start()]
            if prefix.count("{") == prefix.count("}") and match.group(2) in self.bindings:
                self.bindings[match.group(1)] = self.bindings[match.group(2)]
        self.regions: list[tuple[int, int, str, str]] = []
        pattern = r"(?:\b(?:async\s+)?function\s+([\w$]+)\s*\([^)]*\)[^{;]*|\b([\w$]+)\s*:\s*(?:async\s*)?\([^)]*\)\s*=>\s*)\{"
        for match in re.finditer(pattern, self.clean):
            start = match.end() - 1
            end = matching_brace(self.clean, start)
            self.regions.append((match.start(), end + 1, match.group(1) or match.group(2), self.clean[start:end + 1]))
        self.functions = {name: body for _, _, name, body in self.regions}
        self.module_cache: dict[str, set[str]] = {}
        self.identifiers = {name: set(re.findall(r"\b[A-Za-z_$][\w$]*\b", body)) for name, body in self.functions.items()}
        self.calls = {name: set(re.findall(r"(?<![\w$.])([A-Za-z_$][\w$]*)\s*\(", body)) for name, body in self.functions.items()}

    def modules(self, body: str) -> set[str]:
        if body not in self.module_cache:
            names = set(re.findall(r"\b[A-Za-z_$][\w$]*\b", body))
            direct = {self.bindings[name] for name in names if name in self.bindings}
            seen: set[str] = set()
            # An owned handler/render body is the attribution boundary. Generic
            # redraw callers do not make its feature own the entire application.
            calls = set(re.findall(r"(?<![\w$.])([A-Za-z_$][\w$]*)\s*\(", body))
            pending = list(calls & self.functions.keys()) if not direct else []
            while pending:
                name = pending.pop()
                if name in seen:
                    continue
                seen.add(name)
                used = self.identifiers[name]
                names.update(used)
                # A method name or local variable matching another function is
                # not a call edge (e.g. fleet.confirm.action vs function action).
                if not any(identifier in self.bindings for identifier in used):
                    pending.extend(self.calls[name] & self.functions.keys() - seen)
            self.module_cache[body] = {self.bindings[name] for name in names if name in self.bindings} - {"hub-web/src/main.ts", "hub-web/src/styles.css"}
        return self.module_cache[body]

    def usage_modules(self, identifiers: set[str]) -> set[str]:
        modules: set[str] = set()
        for _, _, _, body in self.regions:
            if identifiers & set(re.findall(r"\b[A-Za-z_$][\w$]*\b", body)):
                modules.update(self.modules(body))
        return modules

    def selector_modules(self, anchors: set[str]) -> set[str]:
        modules: set[str] = set()
        for lo, hi, _, body in self.regions:
            if any(re.search(rf"(?<![\w-]){re.escape(a)}(?![\w-])", self.text[lo:hi]) for a in anchors):
                modules.update(self.modules(body))
        return modules


class Impact:
    def __init__(self, root: Path, journeys: list[dict], before: SourceTree | None, after: SourceTree):
        self.root, self.before, self.after, self.journeys = root, before, after, journeys
        self.trees = [tree for tree in (before, after) if tree]
        self.models = [MainModel(tree, "hub-web/src/main.ts") for tree in self.trees]
        self.reachable: dict[str, set[str]] = {}
        self.browser: dict[str, set[str]] = {}
        for journey in journeys:
            ident = journey["id"]
            self.reachable[ident], self.browser[ident] = set(), set()
            for tree in self.trees:
                patterns = globs(journey["fields"].get("Touches", ""))
                roots = {path for path in tree.paths if path.startswith("hub-web/src/") and path not in ("hub-web/src/main.ts", "hub-web/src/styles.css") and not path.endswith(".test.ts") and any(fnmatch.fnmatchcase(path, p) for p in patterns)}
                browser = closure(tree, roots)
                self.browser[ident].update(browser)
                specs = {path for path in tree.paths if path.endswith(".journey.ts") and re.search(rf"['\"`]{re.escape(ident)}\b", tree.read(path))}
                if suite_path(journey):
                    specs.add(suite_path(journey))
                self.reachable[ident].update(closure(tree, specs) | browser)

        # A provider composed only by main.ts has no direct catalog root.
        # Attribute its usage to the other catalog-owned modules in the same
        # render/handler; do not follow every import of the composition root.
        for model in self.models:
            for module in sorted(set(model.bindings.values())):
                if self.users(module):
                    continue
                names = {name for name, path in model.bindings.items() if path == module}
                owners = set().union(*(self.users(path) for path in model.usage_modules(names)))
                for ident in owners:
                    self.reachable[ident].add(module)
                    self.browser[ident].add(module)

    def users(self, path: str) -> set[str]:
        return {j["id"] for j in self.journeys if path in self.reachable[j["id"]] or path not in ("hub-web/src/main.ts", "hub-web/src/styles.css") and any(fnmatch.fnmatchcase(path, p) for p in globs(j["fields"].get("Touches", "")))}

    def css(self, path: str) -> tuple[set[str], str]:
        if not self.before:
            return set(), "surface-wide:css-without-base"
        try:
            old, new = css_rules(self.before.read(path)), css_rules(self.after.read(path))
        except ValueError as error:
            return set(), f"surface-wide:unparsed-css:{error}"
        changed = [key for key in old.keys() | new.keys() if old.get(key) != new.get(key)]
        ids: set[str] = set()
        selectors = []
        for _, selector in changed:
            if global_selector(selector):
                return set(), f"surface-wide:css-base:{selector}"
            anchors = selector_anchors(selector)
            matches: set[str] = set()
            for journey in self.journeys:
                ident = journey["id"]
                # Actual spec literals and explicit browser module roots only;
                # shared hub fixtures do not prove that a page uses a component.
                for tree in self.trees:
                    corpus = "\n".join(tree.read(p) for p in self.browser[ident] | {suite_path(journey) or ""})
                    if any(re.search(rf"(?<![\w-]){re.escape(anchor)}(?![\w-])", corpus) for anchor in anchors):
                        matches.add(ident)
            for model in self.models:
                for module in model.selector_modules(anchors):
                    matches.update(self.users(module))
            if not matches:
                return set(), f"surface-wide:unmapped-css:{selector}"
            ids.update(matches)
            selectors.append(selector)
        return ids, "css-selectors:" + ";".join(sorted(selectors))

    def fixture(self, path: str) -> tuple[set[str], str]:
        if not self.before:
            return set(), f"surface-wide:fixture-without-base:{path}"
        import difflib
        users = self.users(path)
        ids: set[str] = set()
        owners: set[str] = set()
        # Parse both revisions before an early wide/narrow return can bypass
        # an uncertain boundary in the other revision.
        models = [MainModel(tree, path) for tree in (self.before, self.after)]
        for model, other in ((models[0], self.after), (models[1], self.before)):
            tree = self.before if model is models[0] else self.after
            # Class methods form additional ownership boundaries in hub doubles.
            regions = list(model.regions)
            for match in re.finditer(r"^\s*(?:(?:private|public|protected|async|static)\s+)*([\w$]+)\s*\([^;{}]*\)[^;{}]*\{", model.clean, re.MULTILINE):
                if match.group(1) not in ("if", "for", "while", "switch", "catch"):
                    start = match.end() - 1
                    end = matching_brace(model.clean, start)
                    regions.append((match.start(), end + 1, match.group(1), model.text[start:end + 1]))
            lines, other_lines = model.text.splitlines(), other.read(path).splitlines()
            offsets = [0]
            for line in model.text.splitlines(keepends=True):
                offsets.append(offsets[-1] + len(line))
            for tag, start, end, _, _ in difflib.SequenceMatcher(None, lines, other_lines, autojunk=False).get_opcodes():
                if tag == "equal" or start == end:
                    continue
                excerpt = "\n".join(lines[start:end])
                if not mask_comments(excerpt).strip():
                    continue
                lo, hi = offsets[start], offsets[end]
                segment = model.text[lo:hi]
                lo += len(segment) - len(segment.lstrip())
                hi -= len(segment) - len(segment.rstrip())
                recorder_names: set[str] = set()
                for r in regions:
                    if r[0] < hi and r[1] > lo and re.search(r"^(constructor|route|routing|dispatch|received|socket|machineSocket|handleSessionFrame|install)$", r[2]):
                        old_text, new_text = self.before.read(path), self.after.read(path)
                        # Only dispatch methods, never constructors or setup.
                        if r[2] not in ("constructor", "install") and tree is self.after:
                            old_model = MainModel(self.before, path)
                            for match in re.finditer(rf"^\s*(?:(?:private|public|protected|async|static)\s+)*{re.escape(r[2])}\s*\([^;{{}}]*\)[^;{{}}]*\{{", old_model.clean, re.MULTILINE):
                                stop = matching_brace(old_model.clean, match.end() - 1) + 1
                                recorder_names = additive_recorders(old_text, new_text, old_text[match.start():stop], new_text[r[0]:r[1]])
                        if not recorder_names:
                            return set(), f"surface-wide:fixture-core:{path}:{r[2]}"
                containing = [r for r in regions if r[0] <= lo and r[1] >= hi]
                region = min(containing, key=lambda r: r[1] - r[0]) if containing else None
                if region and not recorder_names and re.search(r"^(constructor|route|routing|dispatch|received|socket|machineSocket|handleSessionFrame|install)$", region[2]):
                    return set(), f"surface-wide:fixture-core:{path}:{region[2]}"
                names: set[str] = set()
                keys: set[str] = set()
                if recorder_names:
                    names.update(recorder_names)
                elif region:
                    names.add(region[2])
                    # A guarded optional world owns the operation. Generic
                    # machines/options elsewhere in that method do not broaden it.
                    for match in re.finditer(r"const\s+(\w+)\s*=\s*this\.options\.(\w+)[^;]*;\s*if\s*\(\s*!\1\s*\)\s*return", region[3]):
                        keys.add(match.group(2))
                else:
                    names.update(re.findall(r"\b(?:readonly|function|interface|type|const)\s+(\w+)", mask_comments(excerpt)))
                matches: set[str] = set()
                for journey in self.journeys:
                    if journey["id"] not in users:
                        continue
                    for snapshot in self.trees:
                        specs = {p for p in snapshot.paths if p.endswith(".journey.ts") and re.search(rf"['\"`]{re.escape(journey['id'])}\b", snapshot.read(p))}
                        corpus = "\n".join(snapshot.read(p) for p in closure(snapshot, specs) - {path})
                        if any(re.search(rf"\b{re.escape(key)}\s*:", corpus) for key in keys) or any(re.search(rf"\b{re.escape(name)}\b", mask_comments(corpus)) for name in names):
                            matches.add(journey["id"])
                if not matches:
                    return set(), f"surface-wide:unattributed-fixture:{path}:{start + 1}"
                ids.update(matches)
                owners.update(keys | names)
        return ids, f"fixture-symbols:{path}:" + ",".join(sorted(owners))

    def package(self, path: str) -> bool:
        """True means surface-wide, including unknown lockfile-only changes."""
        if not self.before:
            return True
        try:
            old = json.loads(self.before.read("hub-web/package.json"))
            new = json.loads(self.after.read("hub-web/package.json"))
        except (ValueError, TypeError, AttributeError):
            return True
        if {k: v for k, v in old.items() if k != "devDependencies"} != {k: v for k, v in new.items() if k != "devDependencies"}:
            return True
        if path.endswith("package.json"):
            return False
        if old.get("devDependencies") == new.get("devDependencies"):
            return True
        try:
            locks = [json.loads(tree.read(path)) for tree in (self.before, self.after)]
            if locks[0].get("lockfileVersion") != locks[1].get("lockfileVersion"):
                return True
            pkgs = [lock["packages"] for lock in locks]
            if any(pkgs[i].get("", {}).get("dependencies", {}) != (old, new)[i].get("dependencies", {}) for i in range(2)):
                return True
            for name in pkgs[0].keys() | pkgs[1].keys():
                if name == "":
                    a, b = ({k: v for k, v in x.get("", {}).items() if k != "devDependencies"} for x in pkgs)
                    if a != b:
                        return True
                elif pkgs[0].get(name) != pkgs[1].get(name) and any(not x.get(name, {}).get("dev", False) for x in pkgs if name in x):
                    return True
            # Unknown lockfile metadata cannot be attributed to devDependencies.
            return {k: v for k, v in locks[0].items() if k != "packages"} != {k: v for k, v in locks[1].items() if k != "packages"}
        except (KeyError, ValueError, TypeError, AttributeError):
            return True

    def composition(self, path: str) -> tuple[set[str], str]:
        if not self.before:
            return set(), "surface-wide:composition-without-base"
        import difflib
        ids: set[str] = set()
        for tree, other in ((self.before, self.after), (self.after, self.before)):
            text = tree.read(path)
            lines, other_lines = text.splitlines(), other.read(path).splitlines()
            model = MainModel(tree, path)
            regions = model.regions
            offsets = [0]
            for line in text.splitlines(keepends=True):
                offsets.append(offsets[-1] + len(line))
            for tag, start, end, _, _ in difflib.SequenceMatcher(None, lines, other_lines, autojunk=False).get_opcodes():
                if tag == "equal" or start == end:
                    continue
                excerpt = "\n".join(lines[start:end])
                if not mask_comments(excerpt).strip():
                    continue
                lo, hi = offsets[start], offsets[end]
                imports = [module for a, b, module in model.imports if a < hi and b > lo]
                # Attribute partial/multiline import hunks by the full statement.
                if imports:
                    modules = set(imports)
                else:
                    containing = [region for region in regions if region[0] <= lo and region[1] >= hi]
                    if containing:
                        region = min(containing, key=lambda r: r[1] - r[0])
                        if re.search(r"^(?:bootstrap|init|initialize|startApp|main)$", region[2], re.I):
                            return set(), f"surface-wide:bootstrap:{region[2]}"
                        modules = model.modules(mask_comments(excerpt, strings=True)) or model.modules(region[3])
                    else:
                        # Top-level wiring may still be attributable by imported
                        # identifiers; an unowned init hunk must stay surface-wide.
                        body = mask_comments(excerpt, strings=True)
                        modules = model.modules(body)
                        if not modules:
                            # Attribute declared state by its consumers, never
                            # ubiquitous type words such as Map/string/number.
                            declared = set(re.findall(r"\b(?:const|let|var)\s+([\w$]+)", body))
                            modules = model.usage_modules(declared) if declared else set()
                matches = set().union(*(self.users(module) for module in modules)) if modules else set()
                if not matches:
                    return set(), f"surface-wide:unattributed-composition:{start + 1}"
                ids.update(matches)
        return ids, "composition-identifiers:hub-web/src/main.ts"


def select(paths: list[str], surfaces: dict[str, list[str]], journeys: list[dict], impact: Impact | None = None) -> list[dict]:
    if impact is None:
        root = repo_root()
        impact = Impact(root, journeys, None, SourceTree(root))
    chosen: dict[str, dict] = {}
    def add(ids: set[str], reason: str):
        for journey in journeys:
            if journey["id"] in ids and journey["id"] not in chosen:
                chosen[journey["id"]] = row(journey, reason)
    hub_ids = {j["id"] for j in journeys if j["surface"] == "hub-web"}
    for path in paths:
        if path == CATALOG:
            add({j["id"] for j in journeys}, "surface-wide:catalog-change")
            continue
        if path.startswith("hub-web/dist/") or path.endswith((".test.ts", ".spec.ts")) and path.startswith("hub-web/src/"):
            continue
        if path in ("hub-web/package.json", "hub-web/package-lock.json"):
            if impact.package(path):
                add(hub_ids, f"surface-wide:runtime-or-build-package:{path}")
            continue
        if path == "hub-web/src/styles.css":
            ids, reason = impact.css(path)
            add(hub_ids if reason.startswith("surface-wide:") else ids, reason)
            continue
        if path == "hub-web/src/main.ts":
            ids, reason = impact.composition(path)
            add(hub_ids if reason.startswith("surface-wide:") else ids, reason)
            continue
        if path.startswith("hub-web/e2e/") and path.endswith((".ts", ".js", ".mjs")) and not path.endswith(".journey.ts"):
            ids, reason = impact.fixture(path)
            add(hub_ids if reason.startswith("surface-wide:") else ids, reason)
            continue
        if path.startswith("hub-web/src/") and path.endswith((".ts", ".tsx", ".js", ".mjs")) or path.startswith("hub-web/e2e/"):
            users = impact.users(path)
            for journey in journeys:
                if journey["id"] in users:
                    direct = next((p for p in globs(journey["fields"].get("Touches", "")) if fnmatch.fnmatchcase(path, p)), None)
                    add({journey["id"]}, "suite" if path == suite_path(journey) else direct or f"import-graph:{path}")
            if not users:
                add(hub_ids, f"surface-wide:unmapped-source:{path}")
            continue
        if path.startswith("hub-web/src/") and path.endswith(".css"):
            add(hub_ids, f"surface-wide:tokens-or-unmapped-css:{path}")
            continue
        if path in ("hub-web/vite.config.ts", "hub-web/tsconfig.json", "hub-web/e2e/tsconfig.json"):
            add(hub_ids, f"surface-wide:build-config:{path}")
            continue
        for surface, wide in surfaces.items():
            if any(fnmatch.fnmatchcase(path, p) for p in wide):
                add({j["id"] for j in journeys if j["surface"] == surface}, f"surface-wide:{path}")
        for journey in journeys:
            if path == suite_path(journey):
                add({journey["id"]}, "suite")
            elif any(fnmatch.fnmatchcase(path, p) for p in globs(journey["fields"].get("Touches", ""))):
                add({journey["id"]}, "catalog-touch:" + path)
    return [chosen[j["id"]] for j in journeys if j["id"] in chosen]

def row(journey: dict, reason: str) -> dict:
    return {
        "id": journey["id"],
        "title": journey["title"],
        "surface": journey["surface"],
        "suite": suite_path(journey),
        "reason": reason,
    }


def main(argv: list[str]) -> int:
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__.strip())
        return 0 if argv else 2
    root = repo_root()
    revision = os.environ.get("CAS_JOURNEYS_HEAD")
    base = os.environ.get("CAS_JOURNEYS_BASE")
    if argv[0] not in ("--paths", "--all", "--check"):
        base, revision = argv[0], argv[1] if len(argv) > 1 else revision or "HEAD"
    tree = SourceTree(root, revision)
    catalog = root / CATALOG
    if CATALOG not in tree.paths:
        sys.stderr.write(f"journeys: no catalog at {catalog}\n")
        return 2
    surfaces, journeys = parse(tree.read(CATALOG))
    if argv[0] != "--check" and (not journeys or len({j["id"] for j in journeys}) != len(journeys) or any(not ID_RE.match(j["id"]) for j in journeys)):
        sys.stderr.write("journeys: empty or invalid catalog; refusing selection\n")
        return 2
    if argv[0] == "--check":
        errors = check(root, surfaces, journeys, tree)
        for error in errors:
            print(f"journeys: {error}", file=sys.stderr)
        if errors:
            return 1
        print(f"journeys: catalog OK ({len(journeys)} journeys)")
        return 0
    before = None
    if base:
        comparison_base = base
        if revision:
            comparison_base = subprocess.run(["git", "-C", str(root), "merge-base", base, revision], capture_output=True, text=True, check=True).stdout.strip()
        before = SourceTree(root, comparison_base)
    try:
        selected_impact = Impact(root, journeys, before, tree) if argv[0] != "--all" else None
        if argv[0] == "--all":
            selected = [row(j, "all") for j in journeys]
        elif argv[0] == "--paths":
            selected = select(argv[1:], surfaces, journeys, selected_impact)
        else:
            selected = select(changed_paths(root, base, revision), surfaces, journeys, selected_impact)
    except ValueError as error:
        # An uncertain lexical/brace boundary cannot justify a narrow or empty
        # selection. Git/catalog failures still refuse outside this boundary.
        selected = [row(j, f"surface-wide:uncertain-source-parser:{error}") for j in journeys]
    print(json.dumps({"catalog": CATALOG, "journeys": selected}, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
