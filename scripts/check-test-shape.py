#!/usr/bin/env python3
"""Deterministic Rust test-shape lint; no compiler or third-party packages.

Scan test bodies and test-only helpers for constant/literal equality pins and
Rust source reads. A // pin: reason immediately above the statement or function
acknowledges an intentional structural/compatibility contract. Vendor code is
excluded. --changed-since uses merge-base(ref, HEAD), including working changes.
"""
import argparse
from pathlib import Path
import re
import subprocess
from rust_test_source import tokens, pairs


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


def lint(source, path):
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
    seen = set()
    for fn, begin, end in scopes:
        function_pin = fn >= 0 and pinned(source, ts[fn - 1].start if fn > 0 and ts[fn - 1].value == "async" else ts[fn].start)
        for i in range(begin + 1, end):
            t = ts[i]
            if t.string:
                continue
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
    return sorted(hits)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--changed-since')
    args = parser.parse_args()
    try:
        if args.changed_since:
            base = subprocess.check_output(['git', 'merge-base', args.changed_since, 'HEAD'], text=True).strip()
            paths = subprocess.check_output(['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', base, '--', '*.rs'], text=True).split('\0')
        else:
            paths = subprocess.check_output(['git', 'ls-files', '-z', '*.rs'], text=True).split('\0')
        hits = []
        for path in sorted(set(paths)):
            if not path or path.startswith('vendor/') or not Path(path).is_file():
                continue
            for line, kind in lint(Path(path).read_text(), path):
                hits.append(f'{path}:{line}: {kind}; add // pin: <reason> or replace with a behavior test')
        print('\n'.join(hits) if hits else 'test-shape: PASS (no unpinned hits)')
        return bool(hits)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'test-shape: ERROR: {error}')
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
