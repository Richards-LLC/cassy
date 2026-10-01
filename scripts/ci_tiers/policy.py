#!/usr/bin/env python3
"""CI policy assertions over parsed data, with independent executable selectors.

Step names are display text. IDs, actions, command anchors and directories bind
our contracts to executable steps. Public job/status names remain data contracts.
Prose pins are intentionally separate and never establish executable wiring.
"""
import argparse
import copy
import json
import re
import sys
from functools import lru_cache
from pathlib import Path

try:
    import yaml
except ImportError:
    raise SystemExit("CI policy parser requires pyyaml==6.0.3; install with python3 -m pip install -r scripts/ci_tiers/requirements.txt")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


class Loader(yaml.SafeLoader):
    pass


# YAML 1.2: GitHub's `on` key must not become the YAML 1.1 boolean True.
Loader.yaml_implicit_resolvers = copy.deepcopy(Loader.yaml_implicit_resolvers)
for initial, resolvers in Loader.yaml_implicit_resolvers.items():
    Loader.yaml_implicit_resolvers[initial] = [(tag, regex) for tag, regex in resolvers if tag != 'tag:yaml.org,2002:bool']
Loader.add_implicit_resolver('tag:yaml.org,2002:bool', re.compile(r'^(?:true|false)$', re.I), list('tTfF'))


def uncomment(text):
    """Discard comment tokens, respecting quoted strings and escaped hashes.

    A hash inside a word/parameter expansion or printed string is data. A hash
    at an unquoted word boundary starts a comment, including inline comments.
    """
    result = []
    quote = None
    escaped = False
    comment = False
    previous = '\n'
    for char in text:
        if comment:
            if char == '\n':
                comment = False
                result.append(char)
            previous = char
            continue
        if escaped:
            result.append(char)
            escaped = False
        elif char == '\\' and quote != "'":
            result.append(char)
            escaped = True
        elif quote:
            result.append(char)
            if char == quote:
                quote = None
        elif char in ("'", '"'):
            result.append(char)
            quote = char
        elif char == '#' and (previous.isspace() or previous in ';|&()'):
            comment = True
        else:
            result.append(char)
        previous = char
    return ''.join(result)


@lru_cache(maxsize=None)
def load(path):
    text = path.read_text()
    return json.loads(text) if path.suffix == '.json' else yaml.load(text, Loader=Loader)


def scalar_text(value):
    if value is True: return 'true'
    if value is False: return 'false'
    if value is None: return ''
    return str(value)


def matches(step, selector):
    for key, expected in selector.items():
        if key == 'run_contains':
            if expected not in uncomment(step.get('run', '')): return False
        elif key == 'with':
            if any(step.get('with', {}).get(k) != v for k, v in expected.items()): return False
        elif step.get(key) != expected: return False
    # An id/metadata-only placeholder is not a wired step.
    return bool(uncomment(step.get('run', '')).strip() or step.get('uses'))


def resolve(root, source):
    if 'files' in source: return [resolve(root, {'file': f}) for f in source['files']]
    path = root / source['file']
    if path.suffix not in ('.yml', '.yaml', '.json'): return uncomment(path.read_text())
    node = load(path)
    if source.get('action_run'):
        return '\n'.join(step['run'] for step in node['runs']['steps'] if 'run' in step)
    if 'job' in source: node = node['jobs'][source['job']]
    for key in source.get('path', []): node = node[key]
    if 'step' in source:
        found = [s for s in node['steps'] if matches(s, source['step'])]
        if len(found) != 1: raise ValueError(f'expected one wired step, found {len(found)}: {source}')
        node = found[0]
    return node


def nodes(value):
    yield value
    if isinstance(value, dict):
        for key, child in value.items():
            # A step display label cannot satisfy a command/wiring assertion.
            if key == 'name' and ('run' in value or 'uses' in value): continue
            yield from nodes(child)
    elif isinstance(value, list):
        for child in value: yield from nodes(child)


def equivalent(actual, expected):
    if isinstance(actual, str) and isinstance(expected, str):
        return ' '.join(actual.split()) == ' '.join(expected.split())
    if type(actual) is not type(expected): return False
    if isinstance(actual, list): return len(actual) == len(expected) and all(equivalent(a, e) for a, e in zip(actual, expected))
    if isinstance(actual, dict): return actual.keys() == expected.keys() and all(equivalent(actual[k], expected[k]) for k in actual)
    return actual == expected


def count_matches(value, needle):
    # Recognize typed YAML/JSON fields and lists instead of their spelling.
    try: expected = yaml.load(needle.strip(), Loader=Loader)
    except yaml.YAMLError: expected = needle
    if isinstance(expected, dict) and all(re.fullmatch(r'[A-Za-z0-9_-]+', str(k)) for k in expected):
        total = 0
        for node in nodes(value):
            if not isinstance(node, dict): continue
            if all(k in node and (v is None or (isinstance(v, str) and isinstance(node[k], str) and k == 'if' and ' '.join(v.split()) in ' '.join(node[k].split())) or equivalent(node[k], v)) for k, v in expected.items()): total += 1
        return total
    if isinstance(expected, list):
        return sum(sum(equivalent(item, expected[0]) for item in node) for node in nodes(value) if isinstance(node, list))
    # Fragments are limited to actual scalar values, including run/if bodies;
    # YAML comments and step display labels never participate.
    normalized = needle.strip()
    return sum(uncomment(scalar_text(node)).count(normalized) for node in nodes(value) if not isinstance(node, (dict, list)))


def check(root, entry, prose=False):
    try:
        if prose and entry['kind'] == 'annotation':
            return bool(entry.get('consumer') and entry.get('needle') and entry.get('reason')), ''
        if prose:
            actual = (root / entry['source']['file']).read_text().count(entry['needle'])
        else:
            subject = resolve(root, entry['source'])
            if entry['kind'] == 'step_exists': return True, ''
            actual = count_matches(subject, entry['needle'])
        wanted = entry.get('expected')
        ok = actual == wanted if entry['kind'] == 'count' else (actual == 0 if entry['kind'] == 'absent' else actual > 0)
        return ok, f'found {actual}, expected {wanted if wanted is not None else entry["kind"]}: {entry.get("needle", "")}'
    except (KeyError, ValueError, OSError, yaml.YAMLError) as error:
        return False, str(error)


def validate(root, verbose=True):
    failures = []
    count = 0
    for name, prose in [('policy.json', False), ('prose-pins.json', True)]:
        for entry in json.loads((HERE / name).read_text()):
            ok, why = check(root, entry, prose)
            count += 1
            if not ok: failures.append((entry, why))
            if verbose: print(('ok   ' if ok else 'FAIL ') + entry['consumer'] + ('' if ok else ': ' + why))
    if verbose: print(f'parsed policy/prose: {count-len(failures)} passed; {len(failures)} failed')
    return failures


def role(block, name, source):
    choices = [s for s in json.loads((HERE / 'step-roles.json').read_text()).get(name, []) if s['file'] == source['file'] and s['job'] == source['job']]
    found = [(i, step) for i, step in enumerate(block.get('steps', [])) if any(matches(step, s['step']) for s in choices)]
    if len(found) != 1: raise ValueError(f'expected one executable role {name!r}, found {len(found)}')
    return found[0]



def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('mode', choices=['check','job','position','action-run','release-run','scalars'])
    parser.add_argument('args', nargs='*')
    args=parser.parse_args()
    if args.mode=='check': return bool(validate(args.root))
    if args.mode=='scalars':
        for file in args.args:
            for node in nodes(resolve(args.root, {'file': file})):
                if isinstance(node, str): print(uncomment(node))
        return 0
    if args.mode=='job':
        source={'file':args.args[0],'job':args.args[1]}
        # Retain source identity separately from display labels.
        print(json.dumps({'source':source,'node':resolve(args.root,source)}));return 0
    if args.mode == 'position':
        data=json.loads(sys.stdin.read());i,step=role(data['node'],args.args[0],data['source'])
        print(i);return 0
    if args.mode=='action-run': print(resolve(args.root,{'file':args.args[0],'action_run':True}));return 0
    source={'file':'.github/workflows/release.yml','job':'release'}
    _,step=role(resolve(args.root,source),'Create Release',source)
    print(step['run']);return 0


if __name__=='__main__': sys.exit(main())
