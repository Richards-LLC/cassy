#!/usr/bin/env python3
"""Deterministic test-shape lint; no compiler or third-party packages.

Scan Rust tests for constant/literal equality pins and Rust source reads, and
test files in any language for exact prose snapshots or source/workflow text
needles. Prose pins cannot be waived by comments. Vendor code is excluded. Integration tests under cas-cli/tests and
crates/*/tests must resolve Cargo binaries at runtime rather than through
env!("CARGO_BIN_EXE_<name>"); this archive-portability rule cannot be exempted.
Text-pin checks cover changed test files, so legacy pins do not block unrelated
work. With --changed-since, only added or modified lines are checked for text
pins; without it, changed test files are scanned in full. --changed-since uses
merge-base(ref, HEAD), including working changes, excluding files imported only
by a released-trunk sync merge.
"""
import argparse
import ast
from pathlib import Path
import re
import subprocess
from rust_test_source import tokens, pairs


def human_message_literal(value):
    """Recognize structured machine data without consulting English vocabulary.

    Rule markers and gate-row records are structured identifiers, even when
    their wire spelling includes spaces. No English vocabulary is consulted.
    """
    if not isinstance(value, str) or not value.strip():
        return False
    static = re.sub(r'\$?\{[^{}]*\}', '', value)
    if not any(char.isalpha() for char in static):
        return False  # Controls and interpolation-only payloads do not prescribe words.
    machine = (re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z', value)
               or re.fullmatch(r'[a-z][\w-]*:[A-Za-z0-9_.-]+', value)
               or re.fullmatch(r'[\w./\\-]+:\d+(?::\d+)?', value)
               or re.fullmatch(r'[A-Za-z_][\w.-]*=[A-Za-z0-9_.-]+', value)
               or re.fullmatch(r'HTTP [1-5]\d{2}', value)
               or re.fullmatch(r'\.[\w.-]+', value)
               or re.fullmatch(r'\[rule:\s*[\w.-]+\]', value)
               or re.fullmatch(r'(?:PASS|FAIL|SKIP) [\w.-]+', value)
               or re.fullmatch(r'[0-9]+|[A-Z][A-Z0-9_]+|[A-Za-z0-9]+(?:[_.-][A-Za-z0-9]+)+', value)
               or re.fullmatch(r'\$[A-Z][A-Z0-9_]*|[0-9a-fA-F]{7,40}', value)
               or (not re.search(r'\s', value) and ('/' in value or value.startswith('--'))))
    return not machine


def python_literal_value(node):
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return ''.join(part.value for part in node.values if isinstance(part, ast.Constant) and isinstance(part.value, str))
    return None


def machine_id_receiver(expression):
    return re.search(r'\.(?:code|error_code|rule_id)(?:\(\))?(?=\s*(?:,|\)|$))', expression) is not None


def message_assertions(source, path, changed_lines=None):
    """Reject prose operands on error channels, never diagnostic arguments."""
    if changed_lines is not None:
        lines = source.splitlines()
        if not any(0 < line <= len(lines) and re.search(r'[\"\'`]', lines[line - 1]) for line in changed_lines):
            return []
    hits = []
    channels = r'(?:\b|_)(?:error|err|exc|exception|stderr|stdout|refusal|detail|remediation|warning|reason)(?:\b|_)|\.(?:message|rejects)\b|\.(?:unwrap_err|expect_err)\('
    if Path(path).suffix in ('.py', '.pyi'):
        try:
            tree = ast.parse(source)
        except SyntaxError:
            return []
        for node in ast.walk(tree):
            if isinstance(node, ast.Assert) and isinstance(node.test, ast.Compare):
                operands = [node.test.left, *node.test.comparators]
                for literal in operands:
                    if not human_message_literal(python_literal_value(literal)):
                        continue
                    actual = ' '.join(ast.get_source_segment(source, item) or '' for item in operands if item is not literal)
                    if re.search(channels, actual) and not machine_id_receiver(actual) and (changed_lines is None or literal.lineno in changed_lines):
                        hits.append((literal.lineno, 'TEST_ASSERTION_PROSE'))
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                continue
            name = node.func.attr
            if name == 'assertRaisesRegex' and len(node.args) >= 2:
                literal = node.args[1]
                receiver = 'error'
            elif name in ('assertIn', 'assertNotIn', 'assertRegex', 'assertNotRegex', 'assertEqual') and len(node.args) >= 2:
                literal, actual = (node.args[0], node.args[1]) if name in ('assertIn', 'assertNotIn') else (node.args[1], node.args[0])
                if name == 'assertEqual' and isinstance(node.args[0], ast.Constant):
                    literal, actual = node.args[0], node.args[1]
                receiver = ast.get_source_segment(source, actual) or ''
            else:
                continue
            value = python_literal_value(literal)
            if isinstance(value, str) and 'Regex' in name:
                grammar = re.sub(r'\\.|\[(?:\\.|[^\]])*\]|\{\d+(?:,\d*)?\}', '', value)
                if not re.search(r'[A-Za-z]', grammar):
                    continue  # Shape/encoding checks do not prescribe words.
            if (human_message_literal(value)
                    and re.search(channels, receiver) and not machine_id_receiver(receiver)
                    and (changed_lines is None or literal.lineno in changed_lines)):
                hits.append((literal.lineno, 'TEST_ASSERTION_PROSE'))
        return hits
    if Path(path).suffix == '.rs':
        ts = tokens(source)
        ps = pairs(ts)
        test_ranges = []
        test_file = is_test_file(path)
        if not test_file:
            for begin, end in ps.items():
                if ts[begin].value == '{' and 'cfg(test)' in ''.join(t.value for t in ts[max(0, begin - 15):begin]):
                    test_ranges.append((ts[begin].start, ts[end].end))
            for index, item in enumerate(ts):
                if item.string or item.value != 'fn':
                    continue
                previous = index - 1
                while previous >= 0 and ts[previous].value not in ('}', ';', '{'):
                    previous -= 1
                attributes = ''.join(t.value for t in ts[previous + 1:index])
                if not ('#[test]' in attributes or re.search(r'::test(?:\([^\]]*\))?\]', attributes) or 'cfg(test)' in attributes):
                    continue
                body = next((j for j in range(index + 1, len(ts)) if not ts[j].string and ts[j].value in ('{', ';')), None)
                if body is not None and ts[body].value == '{':
                    test_ranges.append((ts[body].start, ts[ps[body]].end))
        for i, token in enumerate(ts[:-2]):
            if token.string or token.value not in ('assert', 'assert_eq', 'assert_ne') or ts[i + 1].value != '!' or i + 2 not in ps:
                continue
            if not test_file and not any(left <= token.start < right for left, right in test_ranges):
                continue
            close = ps[i + 2]
            end = ts[close].start
            diagnostic = next(iter(rust_assertion_diagnostics(source[token.start:ts[close].end])), None)
            if diagnostic:
                end = token.start + diagnostic[0]
            expression = source[token.start:end]
            if not re.search(channels, re.sub(r'"(?:\\.|[^"\\])*"', '""', expression)):
                continue
            for literal_index, literal in enumerate(ts[i + 3:close], start=i + 3):
                if not literal.string or literal.start >= end:
                    continue
                prefix = source[token.start:literal.start]
                substring = re.search(r'\.(?:contains|starts_with|ends_with)\(\s*(?:&?format!\(\s*)?$', prefix)
                depth = sum(1 if t.value in ('(', '[', '{') else -1 if t.value in (')', ']', '}') else 0
                            for t in ts[i + 2:literal_index] if not t.string)
                scalar = token.value in ('assert_eq', 'assert_ne') and depth == 1
                if machine_id_receiver(prefix) or (not substring and not scalar):
                    continue
                value = literal.value
                # Character operands are protocol/format controls, not prose.
                if source[literal.start:literal.end].startswith(("'", "b'")):
                    continue
                if value.startswith(('"', 'b"')):
                    try:
                        value = ast.literal_eval(value)
                    except (ValueError, SyntaxError):
                        pass
                line = source.count('\n', 0, literal.start) + 1
                if human_message_literal(value) and (changed_lines is None or line in changed_lines):
                    hits.append((line, 'TEST_ASSERTION_PROSE'))
        return sorted(set(hits))
    if not is_test_file(path):
        return []
    for start, end, value in text_literals(source, path):
        line = source.count('\n', 0, start) + 1
        if source[start:start + 1] == '/':
            literal_parts = re.sub(r'\\.|\[(?:\\.|[^\]])*\]|\{\d+(?:,\d*)?\}', '', value)
            if not re.search(r'[A-Za-z]', literal_parts):
                continue
            value = value.removeprefix('^').removesuffix('$')
        if not human_message_literal(value) or (changed_lines is not None and line not in changed_lines):
            continue
        node_calls = list(re.finditer(r'\bassert\.(?:ok|equal|strictEqual|deepEqual|deepStrictEqual|match|throws|rejects)\(', source[:start]))
        beginning = max(source.rfind('expect(', 0, start), node_calls[-1].start() if node_calls else -1)
        if beginning < source.rfind(';', 0, start) or beginning < 0:
            continue
        context = source[beginning:start]
        if machine_id_receiver(context):
            continue
        if (re.search(r'\.toThrow(?:Error)?\(\s*(?:new\s+[\w.]+\(\s*)?$', context)
                or (re.match(r'assert\.(?:throws|rejects)\(', context)
                    and context.count('(') - context.count(')') == 1 and re.search(r',\s*$', context))
                or (re.search(channels, context) and re.match(r'assert\.(?:ok|equal|strictEqual|deepEqual|deepStrictEqual|match)\(', context)
                    and re.search(r'(?:,|\.includes\()\s*$', context))
                or (re.search(r'\.to(?:MatchObject|Equal|StrictEqual)\(', context)
                    and re.search(r'\b(?:message|reason|remediation|warning)\s*:\s*$', context))
                or (re.search(r'\bexpect\(', context) and re.search(channels, context)
                    and re.search(r'\.to(?:Be|Equal|Contain|Match)\(\s*$', context))):
            hits.append((line, 'TEST_ASSERTION_PROSE'))
    return hits


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
        any(part in ('test', 'tests', '__tests__', 'spec', 'specs', 'e2e') for part in parts[:-1])
        or name.startswith(('test_', 'test-'))
        or (suffix in source_suffixes and re.search(r'(?:[._-])(test|tests|spec|specs|journey)(?:[._-]|$)', name) is not None)
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
    javascript = suffix in {'.js', '.jsx', '.ts', '.tsx', '.mjs', '.cjs'}
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
        if javascript and source[i] == '/' and source[:i].rstrip()[-1:] in ('(', ',', '=', '[', ':'):
            start = i
            i += 1
            body_start = i
            character_class = False
            while i < len(source) and source[i] != '\n':
                if source[i] == '\\':
                    i += 2
                    continue
                if source[i] == '[':
                    character_class = True
                elif source[i] == ']':
                    character_class = False
                elif source[i] == '/' and not character_class:
                    body = source[body_start:i]
                    i += 1
                    while i < len(source) and source[i].isalpha():
                        i += 1
                    yield start, i, body
                    break
                i += 1
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


def is_exact_text_pin(body, context):
    """Recognize long exact prose and short needles into source-like inputs."""
    if ('/' in body and not re.search(r'\s', body)
            or re.fullmatch(r'[A-Z][A-Z0-9_]*=[A-Za-z0-9_.-]+', body)):
        return False  # Machine resource routes and key-value records.
    words = re.findall(r"[A-Za-z0-9_'-]+", body)
    prose = len(body.strip()) >= 55 and len(words) >= 8 and (
        '\n' in body or re.search(r'[.!?](?:\s|$)', body) is not None
    )
    assertion = re.search(
        r'\b(?:assert\w*|expect|toBe|toEqual|toContain|toMatch|toHaveText|'
        r'toMatchSnapshot|snapshot|should)\b|(?:===|==)', context, re.I,
    ) is not None
    source_input = re.search(
        r'\b(?:read_text|readFile|read_to_string|include_str)\b|'
        r'(?<![\w.])(?:source|prompt|workflow|skill|template|contract|markdown|yaml|toml)(?:[_A-Za-z0-9]*)\b', context, re.I,
    ) is not None
    search = re.search(
        r'\b(?:contains|includes|indexOf|find|assertIn|assertRegex|toContain|'
        r'toMatch|toEqual|toBe|assert\w*)\b|(?:===|==)', context, re.I,
    ) is not None
    return (prose and assertion) or (len(body.strip()) >= 12 and len(words) >= 2 and source_input and search)


def rust_assertion_diagnostics(source):
    """Yield formatting-argument spans, leaving assertion operands in scope."""
    ts = tokens(source)
    ps = pairs(ts)
    for i, token in enumerate(ts):
        if token.string or token.value not in (
            'assert', 'assert_eq', 'assert_ne',
            'debug_assert', 'debug_assert_eq', 'debug_assert_ne',
        ):
            continue
        if i + 2 not in ps or ts[i + 1].value != '!':
            continue
        close = ps[i + 2]
        operands = 1 if token.value in ('assert', 'debug_assert') else 2
        j = i + 3
        arg_start, angles = j, 0
        while j < close:
            if not ts[j].string:
                # Type arguments can contain commas outside (), [] and {}.
                if ts[j].value == '<' and (angles or j == arg_start or ts[j - 1].value == '::'):
                    angles += 1
                elif ts[j].value == '>' and angles and ts[j - 1].value != '-':
                    angles -= 1
                elif ts[j].value == ',' and not angles:
                    operands -= 1
                    arg_start = j + 1
                    if operands == 0:
                        yield ts[j].end, ts[close].start
                        break
            if j in ps:
                j = ps[j]
            j += 1


def rust_runtime_inputs(source):
    """Git command arguments and Result/Option expect messages are inputs,
    not assertions about the returned text. Keep surrounding assertions in scope.
    """
    ts = tokens(source)
    ps = pairs(ts)
    for i, token in enumerate(ts[:-1]):
        if token.string or ts[i + 1].value != '(' or i + 1 not in ps:
            continue
        if token.value == 'git' or (
            token.value == 'expect' and i > 0 and ts[i - 1].value == '.'
        ):
            yield ts[i + 1].end, ts[ps[i + 1]].start


def js_assertion_diagnostics(source, path):
    """expect(actual, diagnostic) asserts its first argument only."""
    masked = list(source)
    for start, end, _ in text_literals(source, path):
        masked[start:end] = ' ' * (end - start)
    code = ''.join(masked)
    for match in re.finditer(r'\bexpect(?:\.(?:soft|poll))?\s*\(', code):
        stack = ['(']
        comma = None
        for index in range(match.end(), len(code)):
            char = code[index]
            if char in '([{':
                stack.append(char)
            elif char in ')]}':
                if not stack or stack[-1] != {')': '(', ']': '[', '}': '{'}[char]:
                    break
                stack.pop()
                if not stack:
                    if comma is not None:
                        yield comma + 1, index
                    break
            elif char == ',' and len(stack) == 1 and comma is None:
                comma = index


def python_runtime_inputs(source):
    """Path literals consumed by file reads are inputs, not returned-text pins."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return
    lines = source.splitlines(keepends=True)
    starts = [0]
    for line in lines:
        starts.append(starts[-1] + len(line))

    def offset(line, byte_column):
        # AST columns count UTF-8 bytes; text_literals uses character offsets.
        return starts[line - 1] + len(lines[line - 1].encode('utf-8')[:byte_column].decode('utf-8'))

    for node in ast.walk(tree):
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and node.func.attr in ('read_text', 'read_bytes')):
            for value in ast.walk(node.func.value):
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    yield offset(value.lineno, value.col_offset), offset(value.end_lineno, value.end_col_offset)


def lint_text_pins(source, path, ranges=None, changed_lines=None):
    """Find brittle prose pins in a conventional test file, independent of language."""
    if not is_test_file(path) and not ranges:
        return []
    hits = []
    literals = list(text_literals(source, path))
    diagnostics = (list(rust_assertion_diagnostics(source)) if Path(path).suffix == '.rs'
                   else list(js_assertion_diagnostics(source, path)) if Path(path).suffix in ('.js', '.jsx', '.ts', '.tsx', '.mjs', '.cjs') else [])
    inputs = list(rust_runtime_inputs(source)) if Path(path).suffix == '.rs' else []
    if Path(path).suffix in ('.py', '.pyi'):
        inputs = list(python_runtime_inputs(source))
    for start, end, body in literals:
        if any(left <= start < right for left, right in diagnostics):
            continue
        if ranges and not any(left <= start < right for left, right in ranges):
            continue
        if any(left <= start < right for left, right in inputs):
            continue
        start_line = source.count('\n', 0, start) + 1
        end_line = source.count('\n', 0, end) + 1
        changed_literal_lines = (
            sorted(line for line in changed_lines if start_line <= line <= end_line)
            if changed_lines is not None else []
        )
        if changed_lines is not None and not changed_literal_lines:
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


def released_trunk():
    """Match task attribution: prefer origin's default trunk, then local-only trunk."""
    remote = subprocess.run(['git', 'symbolic-ref', '--quiet', 'refs/remotes/origin/HEAD'],
                            capture_output=True, text=True)
    prefix = 'refs/remotes/origin/'
    branches = ([remote.stdout.strip()[len(prefix):]]
                if remote.returncode == 0 and remote.stdout.strip().startswith(prefix)
                else ['main', 'master'])
    for branch in branches:
        for ref in [f'refs/remotes/origin/{branch}', f'refs/heads/{branch}']:
            if subprocess.run(['git', 'show-ref', '--verify', '--quiet', ref]).returncode == 0:
                return ref
    return None


def task_changed_paths(base):
    """Keep task edits on either side of syncs, including tracked working edits."""
    trunk = released_trunk()
    if trunk and subprocess.run(['git', 'merge-base', '--is-ancestor', 'HEAD', trunk]).returncode == 0:
        # Released trunk CI must check the whole diff, not waive its own merges.
        trunk = None
    history = subprocess.check_output(
        ['git', 'rev-list', '--first-parent', '--parents', f'{base}..HEAD'], text=True,
    )
    paths = set()
    for row in history.splitlines():
        commit, first, *side = row.split()
        if trunk and side and all(subprocess.run(
            ['git', 'merge-base', '--is-ancestor', parent, trunk],
        ).returncode == 0 for parent in side):
            # Like cas-985f, the released side parents vouch for the sync,
            # including its resolutions. An unrecognized merge stays checked.
            continue
        paths.update(subprocess.check_output(
            ['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', first, commit], text=True,
        ).split('\0'))
    paths.update(subprocess.check_output(
        ['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', 'HEAD'], text=True,
    ).split('\0'))
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--changed-since')
    parser.add_argument('--messages-only', action='store_true')
    args = parser.parse_args()
    try:
        if args.changed_since:
            base = subprocess.check_output(['git', 'merge-base', args.changed_since, 'HEAD'], text=True).strip()
            paths = subprocess.check_output(['git', 'diff', '--name-only', '--diff-filter=ACMR', '-z', base], text=True).split('\0')
            owned = task_changed_paths(base)
            paths = [path for path in paths if path in owned]
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
                file_hits = [] if args.messages_only else lint(source, path, path in text_pin_paths,
                                 changed_text_lines.get(path) if changed_text_lines is not None else None)
            else:
                file_hits = [] if args.messages_only else lint_text_pins(
                    source, path,
                    changed_lines=changed_text_lines.get(path) if changed_text_lines is not None else None,
                )
            file_hits.extend(message_assertions(source, path,
                changed_text_lines.get(path) if changed_text_lines is not None else None))
            for line, kind in file_hits:
                if kind == 'archive-unsafe Cargo binary path':
                    remedy = 'use cas::test_paths::cas_binary() for archive runners'
                elif kind.startswith('archive-unsafe Cargo binary path ('):
                    name = kind[kind.index('(') + 1:-1]
                    remedy = (f'resolve it at runtime: std::env::var_os("NEXTEST_BIN_EXE_{name}"), '
                              f'falling back to option_env!("CARGO_BIN_EXE_{name}")')
                elif kind == 'TEST_ASSERTION_PROSE':
                    remedy = 'assert behavior, an error code or a rule ID; pin comments do not waive prose checks'
                elif kind == 'exact-text pin':
                    remedy = 'assert behavior or machine data; prose pins cannot be waived by comments'
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
