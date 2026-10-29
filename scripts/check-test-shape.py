#!/usr/bin/env python3
"""Deterministic test-shape lint; no compiler or third-party packages.

Scan Rust tests for constant/literal equality pins and Rust source reads, and
test files in any language for exact prose snapshots or source/workflow text
needles. A reasoned // pin: comment immediately above the pinned statement is
the escape. Vendor code is excluded. Integration tests under cas-cli/tests and
crates/*/tests must resolve Cargo binaries at runtime rather than through
env!("CARGO_BIN_EXE_<name>"); this archive-portability rule cannot be exempted.
Text-pin checks cover changed test files, so legacy pins do not block unrelated
work. With --changed-since, only added or modified lines are checked for text
pins; without it, changed test files are scanned in full. --changed-since uses
merge-base(ref, HEAD), including working changes.
"""
import argparse
from pathlib import Path
import re
import subprocess
from rust_test_source import tokens, pairs


def is_test_file(path):
    """Recognize conventional test directories and filenames across languages."""
    parts = Path(path).as_posix().lower().split('/')
    name = parts[-1]
    source_suffixes = {
        '.rs', '.py', '.pyi', '.js', '.jsx', '.ts', '.tsx', '.mjs', '.cjs',
        '.sh', '.bash', '.go', '.rb', '.java', '.kt', '.kts', '.cs', '.php',
        '.swift', '.c', '.cc', '.cpp', '.h', '.hpp', '.lua', '.sql', '.pl',
        '.r', '.ex', '.exs', '.scala', '.clj', '.vue', '.svelte', '.yaml',
        '.yml', '.toml',
    }
    suffix = Path(name).suffix
    return (
        any(part in ('test', 'tests', '__tests__', 'spec', 'specs') for part in parts[:-1])
        or name.startswith(('test_', 'test-'))
        or (suffix in source_suffixes and re.search(r'(?:[._-])(test|spec)(?:[._-]|$)', name) is not None)
    )


def text_literals(source, path):
    """Yield quoted strings while skipping line and block comments."""
    suffix = Path(path).suffix.lower()
    if suffix == '.rs':
        comments = []
        for token in tokens(source, comments):
            if token.string:
                literal = source[token.start:token.end]
                raw = re.match(r'(?:b|c)?r(#{0,255})"', literal)
                if raw:
                    body = literal[raw.end():-(1 + len(raw[1]))]
                else:
                    quote = literal.find('"')
                    if quote < 0:
                        quote = literal.find("'")
                    body = literal[quote + 1:-1] if quote >= 0 else token.value
                yield token.start, token.end, body
        return
    hash_comments = suffix in {'.py', '.pyi', '.sh', '.bash', '.rb', '.pl', '.r', '.yaml', '.yml', '.toml', '.ini'}
    i = 0
    while i < len(source):
        if source.startswith('//', i) or (hash_comments and source[i] == '#'):
            newline = source.find('\n', i)
            i = len(source) if newline < 0 else newline + 1
            continue
        if source.startswith('/*', i):
            close = source.find('*/', i + 2)
            i = len(source) if close < 0 else close + 2
            continue
        quote = source[i]
        if quote not in ('"', "'", '`'):
            i += 1
            continue
        width = 3 if quote != '`' and source.startswith(quote * 3, i) else 1
        start = i
        body_start = i + width
        i = body_start
        while i < len(source):
            if source[i] == '\\':
                i += 2
            elif source.startswith(quote * width, i):
                body = source[body_start:i]
                yield start, i + width, body
                i += width
                break
            else:
                if width == 1 and quote != '`' and source[i] == '\n':
                    break
                i += 1
        else:
            continue


def has_text_pin_reason(source, start):
    """A non-empty pin comment may sit on the statement or its adjacent line."""
    prefix = source[:start]
    lines = prefix.splitlines()
    if lines and re.search(r'//\s*pin:\s*\S', lines[-1]):
        return True
    for line in reversed(lines[:-1]):
        stripped = line.strip()
        if not stripped:
            continue
        if stripped.startswith(('//', '#', '*', '/*')):
            if re.search(r'//\s*pin:\s*\S', stripped) or re.search(r'#\s*pin:\s*\S', stripped):
                return True
            continue
        break
    return False


def is_exact_text_pin(body, context):
    """Recognize long exact prose and short needles into source-like inputs."""
    words = re.findall(r"[A-Za-z0-9_'-]+", body)
    prose = len(body.strip()) >= 55 and len(words) >= 8 and (
        '\n' in body or re.search(r'[.!?](?:\s|$)', body) is not None
    )
    assertion = re.search(
        r'\b(?:assert\w*|expect|toBe|toEqual|toContain|toMatch|toHaveText|'
        r'toMatchSnapshot|snapshot|should)\b|(?:===|==)', context, re.I,
    ) is not None
    source_input = re.search(
        r'\b(?:source|prompt|workflow|skill|template|contract|read_text|readFile|'
        r'read_to_string|include_str|markdown|yaml|toml)(?:[_A-Za-z0-9]*)\b', context, re.I,
    ) is not None
    search = re.search(
        r'\b(?:contains|includes|indexOf|find|assertIn|assertRegex|toContain|'
        r'toMatch|toEqual|toBe|assert\w*)\b|(?:===|==)', context, re.I,
    ) is not None
    return (prose and assertion) or (len(body.strip()) >= 12 and len(words) >= 2 and source_input and search)


def lint_text_pins(source, path, ranges=None, changed_lines=None):
    """Find brittle prose pins in a conventional test file, independent of language."""
    if not is_test_file(path) and not ranges:
        return []
    hits = []
    literals = list(text_literals(source, path))
    for start, end, body in literals:
        if ranges and not any(left <= start < right for left, right in ranges):
            continue
        start_line = source.count('\n', 0, start) + 1
        end_line = source.count('\n', 0, end) + 1
        changed_literal_lines = (
            sorted(line for line in changed_lines if start_line <= line <= end_line)
            if changed_lines is not None else []
        )
        if changed_lines is not None and not changed_literal_lines:
            continue
        if has_text_pin_reason(source, start):
            continue
        line_start = source.rfind('\n', 0, start) + 1
        line_end = source.find('\n', end)
        if line_end < 0:
            line_end = len(source)
        context_chars = list(source[line_start:line_end])
        for literal_start, literal_end, _ in literals:
            if literal_start >= line_end or literal_end <= line_start:
                continue
            left = max(literal_start, line_start) - line_start
            right = min(literal_end, line_end) - line_start
            context_chars[left:right] = ' ' * (right - left)
        context = ''.join(context_chars)
        if is_exact_text_pin(body, context):
            hits.append((changed_literal_lines[0] if changed_literal_lines else start_line, 'exact-text pin'))
    return sorted(set(hits))


def pinned(source, start):
    before = source[:start].splitlines()
    # Current partial line can carry the comment; otherwise walk only the
    # adjacent comment/attribute block, never another statement's reason.
    for line in reversed(before):
        line = line.strip()
        if line.startswith('//') and re.search(r'//\s*pin:\s*\S', line):
            return True
        if line and not line.startswith(('//', '#[')):
            break
    return False


def lint(source, path, check_text_pins=True, changed_text_lines=None):
    comments = []
    ts = tokens(source, comments)
    pin_lines = {source.count("\n", 0, a) + 1 for a, b in comments if re.match(r"//\s*pin:\s*\S", source[a:b])}
    ps = pairs(ts)
    closing = {b: a for a, b in ps.items()}
    integration = '/tests/' in '/' + path or path.startswith('tests/')
    scopes = []
    for i, t in enumerate(ts):
        if t.value != 'fn' or t.string:
            continue
        body = next((j for j in range(i + 1, len(ts)) if not ts[j].string and ts[j].value in ('{', ';')), None)
        if body is None or ts[body].value != '{':
            continue
        # Attribute block since previous item, plus containing cfg(test) scope.
        prev = i - 1
        while prev >= 0 and ts[prev].value not in ('}', ';', '{'):
            prev -= 1
        attrs = ''.join(x.value for x in ts[prev + 1:i])
        ancestors = [a for a, b in ps.items() if ts[a].value == '{' and a < i < b]
        test_module = any('cfg(test)' in ''.join(x.value for x in ts[max(0, a - 15):a]) for a in ancestors)
        if integration or test_module or re.search(r'(?:^|::)test\]', attrs) or '#[test]' in attrs or 'cfg(test)' in attrs:
            scopes.append((i, body, ps[body]))
    # Test-only constants may embed source outside a function. Process them
    # after functions so a function-specific reason remains authoritative.
    if integration:
        scopes.append((-1, -1, len(ts)))
    else:
        scopes.extend((-1, a, b) for a, b in ps.items() if ts[a].value == '{'
                      and 'cfg(test)' in ''.join(x.value for x in ts[max(0, a-15):a]))
    hits = []
    # Check the whole integration source, including constants and helpers.
    # Comments and string examples are already excluded by the Rust tokenizer.
    # A pin cannot repair a producer path on another archive runner.
    # Every workspace crate's integration tests run from the same archive, so
    # the rule covers crates/*/tests and every Cargo binary, not only `cas`.
    if path.startswith('cas-cli/tests/') or re.match(r'crates/[^/]+/tests/', path):
        for i, t in enumerate(ts):
            binary = (re.fullmatch(r'"?CARGO_BIN_EXE_([\w-]+)"?', ts[i + 3].value)
                      if i + 3 < len(ts) and ts[i + 3].string else None)
            if (not t.string and t.value == 'env' and binary
                    and ts[i + 1].value == '!'
                    and ts[i + 2].value in ('(', '[', '{')):
                kind = 'archive-unsafe Cargo binary path'
                if binary[1] != 'cas':
                    kind += f' ({binary[1]})'
                hits.append((source.count('\n', 0, t.start) + 1, kind))
    seen = set()
    for fn, begin, end in scopes:
        function_pin = fn >= 0 and pinned(source, ts[fn - 1].start if fn > 0 and ts[fn - 1].value == "async" else ts[fn].start)
        random_ids = set()
        def random_id(expression):
            code = ''.join(t.value for t in expression if not t.string)
            return bool(re.search(r'(?:\bUuid::new_v4|\brand::random(?:::<[^>]+>)?)\(', code)) or any(
                not t.string and t.value in random_ids for t in expression)
        for i in range(begin + 1, end):
            t = ts[i]
            if t.string:
                continue
            if t.value == 'let':
                start = i + 1 + (ts[i+1].value == 'mut')
                stop = start
                while stop < end and (ts[stop].string or ts[stop].value != ';'):
                    stop = ps[stop] + 1 if stop in ps else stop + 1
                assignment = next((j for j in range(start+1, stop) if ts[j].value == '=' and not ts[j].string), None)
                if assignment is not None and re.fullmatch(r'[A-Za-z_]\w*', ts[start].value):
                    if random_id(ts[assignment+1:stop]):
                        random_ids.add(ts[start].value)
                    else:
                        random_ids.discard(ts[start].value)
            kind = None
            if t.value == 'assert_eq' and [x.value for x in ts[i+1:i+3]] == ['!', '(']:
                close = ps[i + 2]
                args, arg, j = [], [], i + 3
                while j < close:
                    if ts[j].value == ',' and not ts[j].string:
                        args.append(arg)
                        arg = []
                    elif j in ps:
                        arg.extend(ts[j:ps[j]+1])
                        j = ps[j]
                    else:
                        arg.append(ts[j])
                    j += 1
                args.append(arg)
                def const(a):
                    return bool(a) and re.fullmatch(r'(?:[A-Za-z_]\w*::)*[A-Z_][A-Z_0-9]*', ''.join(x.value for x in a)) is not None and not any(x.string for x in a)
                def literal(a):
                    return (len(a) == 1 and (a[0].string or re.fullmatch(r'[0-9][\w.]*|true|false', a[0].value) is not None)) or (len(a) == 2 and a[0].value == '-' and re.fullmatch(r'[0-9][\w.]*', a[1].value) is not None) or re.fullmatch(r'(?:std::time::)?Duration::from_(?:secs|millis|micros|nanos)\([0-9_]+\)', ''.join(x.value for x in a)) is not None
                if len(args) >= 2 and ((const(args[0]) and literal(args[1])) or (literal(args[0]) and const(args[1]))):
                    kind = 'constant-literal equality'
                if len(args) >= 2 and ((random_id(args[0]) and literal(args[1])) or (literal(args[0]) and random_id(args[1]))):
                    kind = 'random ID compared with assertion literal'
            if t.value == 'include_str' and [x.value for x in ts[i+1:i+3]] == ['!', '(']:
                if any(x.string and '.rs' in x.value for x in ts[i+3:ps[i+2]]):
                    kind = 'Rust source as text'
            if t.value in ('read_to_string',) and i + 1 in ps:
                args = ts[i+2:ps[i+1]]
                # A .rs fixture read is still structural and must explain why.
                # Resolve simple local path bindings, rather than flag every
                # unrelated read in a function which happens to create a .rs.
                names = {x.value for x in args if not x.string}
                direct = any(x.string and '.rs' in x.value for x in args)
                for name in names:
                    for j in range(begin, i):
                        if ts[j].value == 'let' and j+1 < i and ts[j+1].value == name:
                            stop = next((k for k in range(j+2, i) if ts[k].value == ';' and not ts[k].string), i)
                            direct |= any(x.string and '.rs' in x.value for x in ts[j:stop])
                if direct:
                    kind = 'Rust source as text'
            if kind and i not in seen:
                seen.add(i)
                # Pin on a let/qualified expression applies to that statement.
                start = i
                while start > begin + 1:
                    previous = start - 1
                    # A semicolon in an array type is not a statement boundary.
                    if not ts[previous].string and ts[previous].value in (')', ']') and previous in closing:
                        start = closing[previous]
                    elif not ts[previous].string and ts[previous].value in (';', '{', '}'):
                        break
                    else:
                        start -= 1
                if not function_pin and not pinned(source, ts[start].start) and not pinned(source, t.start) and source.count("\n", 0, t.start) + 1 not in pin_lines:
                    line = source.count('\n', 0, t.start) + 1
                    hits.append((line, kind))
    text_ranges = []
    if integration:
        text_ranges.append((0, len(source)))
    else:
        for fn, begin, end in scopes:
            first = fn if fn >= 0 else begin
            if first < 0:
                text_ranges.append((0, len(source)))
            elif end < len(ts):
                text_ranges.append((ts[first].start, ts[end].end))
    if check_text_pins:
        hits.extend(lint_text_pins(source, path, text_ranges, changed_text_lines))
    return sorted(hits)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--changed-since')
    args = parser.parse_args()
    try:
        if args.changed_since:
            base = subprocess.check_output(['git', 'merge-base', args.changed_since, 'HEAD'], text=True).strip()
            paths = subprocess.check_output(['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', base], text=True).split('\0')
            text_pin_paths = {path for path in paths if path}
            changed_text_lines = {}
            for path in text_pin_paths:
                diff = subprocess.check_output(
                    ['git', 'diff', '--unified=0', '--no-ext-diff', '--no-color', base, '--', path],
                    text=True,
                )
                lines = set()
                for diff_line in diff.splitlines():
                    hunk = re.match(r'^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@', diff_line)
                    if hunk:
                        first = int(hunk.group(1))
                        count = int(hunk.group(2) or '1')
                        lines.update(range(first, first + count))
                changed_text_lines[path] = lines
        else:
            paths = subprocess.check_output(['git', 'ls-files', '-z', '--', '*.rs'], text=True).split('\0')
            parents = subprocess.check_output(['git', 'rev-list', '--parents', '-n', '1', 'HEAD'], text=True).split()
            if len(parents) > 1:
                changed = subprocess.check_output(
                    ['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', 'HEAD^'], text=True,
                ).split('\0')
            else:
                changed = subprocess.check_output(['git', 'ls-files', '-z'], text=True).split('\0')
            text_pin_paths = set(changed)
            changed_text_lines = None
            paths = [*paths, *changed]
        hits = []
        for path in sorted(set(paths)):
            if (not path or path.startswith('vendor/')
                    or (not path.endswith('.rs') and not is_test_file(path))
                    or not Path(path).is_file()):
                continue
            try:
                source = Path(path).read_text()
            except UnicodeDecodeError:
                continue
            if path.endswith('.rs'):
                file_hits = lint(source, path, path in text_pin_paths,
                                 changed_text_lines.get(path) if changed_text_lines is not None else None)
            else:
                file_hits = lint_text_pins(
                    source, path,
                    changed_lines=changed_text_lines.get(path) if changed_text_lines is not None else None,
                )
            for line, kind in file_hits:
                if kind == 'archive-unsafe Cargo binary path':
                    remedy = 'use cas::test_paths::cas_binary() for archive runners'
                elif kind.startswith('archive-unsafe Cargo binary path ('):
                    name = kind[kind.index('(') + 1:-1]
                    remedy = (f'resolve it at runtime: std::env::var_os("NEXTEST_BIN_EXE_{name}"), '
                              f'falling back to option_env!("CARGO_BIN_EXE_{name}")')
                else:
                    remedy = 'add // pin: <reason> or replace with a behavior test'
                hits.append(f'{path}:{line}: {kind}; {remedy}')
        print('\n'.join(hits) if hits else 'test-shape: PASS (no unpinned hits)')
        # The park refusal quotes FIX lines; an archive-unsafe path has no pin.
        for hit in hits:
            print('FIX: ' + hit)
        return bool(hits)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'test-shape: ERROR: {error}')
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
