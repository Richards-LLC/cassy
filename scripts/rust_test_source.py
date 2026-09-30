"""Dependency-free Rust token spans for deterministic test linters."""
from dataclasses import dataclass
import re


@dataclass
class Token:
    value: str
    start: int
    end: int
    string: bool = False


def tokens(source, comment_spans=None):
    # Consume comments and literals before punctuation: braces/keywords inside
    # raw strings, escaped strings and nested block comments never become code.
    out = []
    i = 0
    while i < len(source):
        if source[i].isspace():
            i += 1
            continue
        if source.startswith('//', i):
            end = source.find('\n', i)
            end = len(source) if end < 0 else end
            if comment_spans is not None:
                comment_spans.append((i, end))
            i = end
            continue
        if source.startswith('/*', i):
            depth = 1
            i += 2
            while i < len(source) and depth:
                if source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            continue
        raw = re.compile(r'(?:b|c)?r(#{0,255})"').match(source, i)
        if raw:
            start = i
            i = raw.end()
            end = source.find('"' + raw[1], i)
            if end < 0:
                raise ValueError('unterminated raw string')
            out.append(Token(source[i:end], start, end + 1 + len(raw[1]), True))
            i = out[-1].end
            continue
        quoted = re.compile(r'(?:b|c)?"|b?\'(?:\\.|[^\'\\])\'').match(source, i)
        if quoted:
            start = i
            if quoted[0].endswith("'"):
                i = quoted.end()
            else:
                i = quoted.end()
                while i < len(source):
                    if source[i] == '\\':
                        i += 2
                    elif source[i] == '"':
                        i += 1
                        break
                    else:
                        i += 1
            out.append(Token(source[start:i], start, i, True))
            continue
        match = re.compile(r'[A-Za-z_][A-Za-z_0-9]*|[0-9][A-Za-z_0-9.]*|::|.', re.S).match(source, i)
        out.append(Token(match[0], i, match.end()))
        i = match.end()
    return out


def pairs(ts):
    stack, result = [], {}
    for i, t in enumerate(ts):
        if t.string:
            continue
        if t.value in ('(', '[', '{'):
            stack.append(i)
        elif t.value in (')', ']', '}'):
            if not stack or ts[stack[-1]].value != {')': '(', ']': '[', '}': '{'}[t.value]:
                raise ValueError('unbalanced Rust delimiters')
            begin = stack.pop()
            result[begin] = i
    if stack:
        raise ValueError('unbalanced Rust delimiters')
    return result

