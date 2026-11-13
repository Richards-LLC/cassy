#!/usr/bin/env python3
"""CHANGELOG edits for the one-command release (scripts/release.sh, cas-f95b).

  release_changelog.py promote <file> <version> <date> <base-version>
      Move everything this branch has not released yet into one dated
      `## [<version>] - <date>` section under an empty `## [Unreleased]`.
      "Not released yet" is the [Unreleased] body plus any section whose
      version is above <base-version>: a section an earlier run of the
      release command promoted under a version another release has since
      taken. Their headings are dropped and their bodies kept, so a re-bump
      renames the section instead of stacking two. Idempotent.
      Per-delivery fragments (changelog.d/*.md beside <file>, except
      README.md; cas-f2be) are unreleased entries too: their bodies lead
      the section in file-name order and the files are deleted. Entries
      under the same `### ` title share one heading, in first-seen order
      (cas-7f95), so same-area fragments cannot fail MD024.

  release_changelog.py rebase <ours-file> <base-file> <merge-base-version> <out-file>
      Resolve a CHANGELOG merge conflict with the base branch: the result is
      the base file with this branch's unreleased entries (as `promote` reads
      them, relative to <merge-base-version>) under its [Unreleased] heading.
      Blocks the base already carries are dropped, so entries that reached
      this branch through an earlier merge of the base are not duplicated.

  release_changelog.py section <file> <version>
      Print the body of the <version> section.

  release_changelog.py verify-deliveries <base-ref> [--notes <draft>] [--report <json>]
      Refuse missing claims and late code on declared constituent epics; print
      task_ids ready for batch_receipt. Inputs come from CAS_RELEASE_CONTEXT_JSON
      or a draft comment: <!-- CAS_RELEASE_INPUTS {"epic_refs":["origin/epic/name"],
      "deferred_tasks":{"cas-xxxx":"reason for excluding this delivery"}} -->.
"""
import argparse
import json
import os
import re
import sqlite3
import sys
import subprocess
from pathlib import Path

HEADING = re.compile(r'^## \[(?P<version>[^\]]+)\]')
UNRELEASED = '## [Unreleased]'
FRAGMENT_DIR = 'changelog.d'


class ChangelogError(Exception):
    pass


def version_key(text):
    if not re.fullmatch(r'\d+\.\d+\.\d+', text or ''):
        return None
    return tuple(int(part) for part in text.split('.'))


def unreleased_span(lines, released_version):
    """Return (heading index, end index) of the unreleased region."""
    try:
        start = lines.index(UNRELEASED)
    except ValueError:
        raise ChangelogError(f'CHANGELOG.md has no "{UNRELEASED}" heading') from None
    floor = version_key(released_version)
    if floor is None:
        raise ChangelogError(f'base version is not x.y.z: {released_version!r}')
    end = start + 1
    while end < len(lines):
        match = HEADING.match(lines[end])
        if match:
            key = version_key(match.group('version'))
            if key is not None and key <= floor:
                break
        end += 1
    return start, end


def tidy(lines):
    out = []
    for line in lines:
        if not line.strip() and (not out or not out[-1].strip()):
            continue
        out.append(line)
    while out and not out[-1].strip():
        out.pop()
    return out


def region_entries(lines, released_version):
    start, end = unreleased_span(lines, released_version)
    return tidy([line for line in lines[start + 1:end] if not HEADING.match(line)]), start, end


def fragment_paths(changelog):
    folder = Path(changelog).parent / FRAGMENT_DIR
    return sorted(path for path in folder.glob('*.md') if path.name != 'README.md')


def grouped(lines):
    """One `### ` heading per section title, in first-seen order (cas-7f95).

    Each fragment brings its own category heading, so three Factory fixes
    otherwise promote as three identical sibling headings and fail MD024.
    Entries keep their text and order within each heading.
    """
    preamble, order, bodies, current = [], [], {}, None
    for line in lines:
        if line.startswith('### '):
            current = line.rstrip()
            if current not in bodies:
                order.append(current)
                bodies[current] = []
            bodies[current].append([])
        elif current is None:
            preamble.append(line)
        else:
            bodies[current][-1].append(line)
    out = tidy(preamble) + ([''] if tidy(preamble) else [])
    for heading in order:
        out += [heading, '']
        for chunk in (tidy(chunk) for chunk in bodies[heading]):
            if not chunk:
                continue
            # Consecutive bullet lists stay one list; other blocks keep a gap.
            if out[-1].strip() and not (chunk[0].startswith('- ') and out[-1].startswith(('- ', '  '))):
                out.append('')
            out += chunk
        out.append('')
    return tidy(out)


def promote(text, version, date, base_version, fragments=()):
    if version_key(version) is None:
        raise ChangelogError(f'release version is not x.y.z: {version!r}')
    lines = text.splitlines()
    entries, start, end = region_entries(lines, base_version)
    folded = [line for fragment in fragments for line in fragment.splitlines() + ['']]
    entries = grouped(tidy(folded + [''] + entries))
    if not entries:
        raise ChangelogError(
            'CHANGELOG.md [Unreleased] has no entries; write the release entries before releasing')
    out = lines[:start + 1] + ['', f'## [{version}] - {date}', ''] + entries + [''] + lines[end:]
    return '\n'.join(out) + '\n'


def blocks(lines):
    current = []
    for line in lines:
        if line.strip():
            current.append(line)
        elif current:
            yield current
            current = []
    if current:
        yield current


def rebase(ours, base, merge_base_version):
    entries, _, _ = region_entries(ours.splitlines(), merge_base_version)
    kept = []
    for block in blocks(entries):
        text = '\n'.join(block)
        kept.append((block, text in base))
    # Drop duplicated blocks; drop a sub-heading only when every block under
    # it was a duplicate, so a generic heading keeps its new entries.
    out = []
    for index, (block, duplicate) in enumerate(kept):
        if duplicate:
            is_heading = block[0].startswith('### ') and len(block) == 1
            if not is_heading:
                continue
            following = []
            for later, later_duplicate in kept[index + 1:]:
                if later[0].startswith('### ') and len(later) == 1:
                    break
                following.append(later_duplicate)
            if all(following):
                continue
        out.extend(block + [''])
    lines = base.splitlines()
    try:
        start = lines.index(UNRELEASED)
    except ValueError:
        raise ChangelogError(f'base CHANGELOG.md has no "{UNRELEASED}" heading') from None
    merged = lines[:start + 1] + ([''] + tidy(out) if tidy(out) else []) + lines[start + 1:]
    return '\n'.join(merged) + '\n'


def section(text, version):
    lines = text.splitlines()
    body = None
    for line in lines:
        match = HEADING.match(line)
        if match:
            if body is not None:
                break
            if match.group('version') == version:
                body = []
            continue
        if body is not None:
            body.append(line)
    if body is None:
        raise ChangelogError(f'CHANGELOG.md has no [{version}] section')
    return '\n'.join(tidy(body)) + '\n'


def verify_deliveries(base_ref, notes=None):
    """Require a non-red delivery in Git history for each explicit task claim."""
    def git(*args):
        result = subprocess.run(['git', *args], capture_output=True, text=True)
        if result.returncode:
            raise ChangelogError(result.stderr.strip())
        return result.stdout.strip()

    owner = re.compile(r'\((cas-[0-9a-f]+)(,? red)?\)$')
    paths = [str(path) for path in fragment_paths('CHANGELOG.md')]
    for commit in git('log', f'{base_ref}..HEAD', '--no-merges', '--format=%H',
                      '--grep=^release: v[0-9]').splitlines():
        paths += git('diff', '--name-only', '--diff-filter=D', f'{commit}^',
                     commit, '--', 'changelog.d/').splitlines()
    tasks = {Path(path).stem for path in paths
             if re.fullmatch(r'cas-[0-9a-f]+\.md', Path(path).name)}
    tasks.update(match[1] for subject in git('log', f'{base_ref}..HEAD', '--no-merges',
                                            '--format=%s').splitlines()
                 if (match := owner.search(subject)))
    deliveries, red = {}, set()
    for line in (git('log', '--all', '--no-merges', '--format=%H%x09%s').splitlines() if tasks else []):
        commit, _, subject = line.partition('\t')
        if match := owner.search(subject):
            if match[2]:
                red.add(commit)
            else:
                deliveries.setdefault(match[1], []).append(commit)
    contained = set(git('rev-list', 'HEAD').splitlines())
    missing = []
    for task in sorted(tasks):
        # Older deliveries identify themselves by adding their own fragment.
        commits = deliveries.get(task) or git('log', '--all', '--no-merges',
            '--diff-filter=A', '--format=%H', '--', f'changelog.d/{task}.md').splitlines()
        commits = [commit for commit in commits if commit not in red]
        if contained.intersection(commits):
            continue
        if any(f'- {commit}' in git('cherry', 'HEAD', commit).splitlines() for commit in commits):
            continue
        missing.append(f'{task}: missing fix commit: {", ".join(commits) or "no non-red delivery found"}')
    if missing:
        raise ChangelogError('release is missing task deliveries:\n' + '\n'.join(missing))

    context = json.loads(os.environ.get('CAS_RELEASE_CONTEXT_JSON') or '{}')
    if not isinstance(context, dict) or not isinstance(context.get('release_inputs') or {}, dict):
        raise ChangelogError('release context and release_inputs must be JSON objects')
    inputs = dict(context.get('release_inputs') or {})
    if notes:
        blocks = re.findall(r'<!--\s*CAS_RELEASE_INPUTS\s+(.*?)-->', Path(notes).read_text(), re.S)
        if len(blocks) > 1:
            raise ChangelogError('declare release inputs once in the draft')
        if blocks:
            declared = json.loads(blocks[0])
            if not isinstance(declared, dict) or set(declared) - {'epic_refs', 'deferred_tasks'}:
                raise ChangelogError('draft inputs accept only epic_refs and deferred_tasks')
            inputs.update(declared)
    refs = inputs.get('epic_refs', [])
    deferred = inputs.get('deferred_tasks', {})
    if not isinstance(refs, list) or not all(isinstance(ref, str) and ref.strip() for ref in refs):
        raise ChangelogError('release epic_refs must be a list of explicit Git refs')
    if not isinstance(deferred, dict) or not all(re.fullmatch(r'cas-[0-9a-f]+', task)
            and isinstance(reason, str) and reason.strip() for task, reason in deferred.items()):
        raise ChangelogError('each deferred task needs a task ID and a non-empty reason')
    refs = sorted(set(refs))
    def branch(ref):
        return re.sub(r'^(?:refs/remotes/[^/]+/|refs/heads/|origin/)', '', ref)
    epic_history = {}
    for ref in refs:
        git('check-ref-format', '--branch', ref)
        resolved = git('rev-parse', '--symbolic-full-name', '--verify', ref)
        if not resolved.startswith(('refs/heads/', 'refs/remotes/')):
            raise ChangelogError(f'constituent epic must be a branch ref: {ref}')
        if resolved.startswith('refs/remotes/'):
            _, _, remote, name = resolved.split('/', 3)
            git('fetch', '--quiet', '--no-tags', remote, f'+refs/heads/{name}:{resolved}')
        epic_history.setdefault(branch(resolved), set()).update(git('rev-list', resolved).splitlines())
    try:
        # Main may already have published this candidate under a newer queue
        # merge. Inventory the candidate's own interval, not main's later tag.
        floor = git('describe', '--tags', '--match', 'v[0-9]*', '--abbrev=0', 'HEAD')
        if git('rev-parse', f'{floor}^{{commit}}') == git('rev-parse', 'HEAD'):
            floor = git('describe', '--tags', '--match', 'v[0-9]*', '--abbrev=0', 'HEAD^')
    except ChangelogError:
        floor = base_ref  # Legacy/local fixture without release tags.
    previous = set(git('rev-list', floor).splitlines())
    included = {match[1] for subject in git('log', f'{floor}..HEAD', '--no-merges', '--format=%s').splitlines()
                if (match := owner.search(subject)) and not match[2]}
    included.update(Path(path).stem for path in git('log', f'{floor}..HEAD', '--format=',
        '--diff-filter=A', '--name-only', '--', 'changelog.d/').splitlines()
        if re.fullmatch(r'cas-[0-9a-f]+\.md', Path(path).name))
    common = Path(git('rev-parse', '--path-format=absolute', '--git-common-dir'))
    root = Path(os.environ.get('CAS_ROOT') or common.parent / '.cas').expanduser().resolve()
    rows = []
    try:
        with sqlite3.connect((root / 'cas.db').as_uri() + '?mode=ro', uri=True) as db:
            rows = db.execute('SELECT id, task_type, deliverables, execution_note, status FROM tasks').fetchall()
            members = set(inputs.get('included_task_ids', []))
            pending = [context['train_id']] if context.get('train_id') else []
            while pending:
                parent = pending.pop()
                children = {row[0] for row in db.execute(
                    "SELECT from_id FROM dependencies WHERE to_id=? AND dep_type='parent-child'", (parent,))}
                pending.extend(children - members)
                members.update(children)
            included.update(task for task, _, _, method, state in rows if method == 'no-code'
                            and (state == 'awaiting_merge' or task in inputs.get('included_task_ids', []))
                            and task in members and task not in deferred)
    except sqlite3.Error as error:
        if refs:
            raise ChangelogError(f'cannot audit declared epics without the project task store ({root}): {error}') from error
    missing = []
    for task, kind, raw, _, _ in rows:
        data = json.loads(raw or '{}')
        if isinstance(data, str):
            data = json.loads(data)
        if kind == 'epic' or not isinstance(data, dict):
            continue
        anchor = data.get('factory_branch_anchor')
        target = branch((data.get('work_target') or {}).get('target_branch') or '')
        if not anchor or not (anchor in contained or anchor in epic_history.get(target, set())):
            continue
        if anchor in previous:
            continue
        if anchor in contained:
            included.add(task)
            continue
        start = (data.get('factory_branch_start') or {}).get('base_sha')
        paths = (git('diff', '--name-only', start, anchor) if start else
                 git('diff-tree', '--root', '-m', '--no-commit-id', '--name-only', '-r', anchor)).splitlines()
        code = any(not path.endswith('.md') and not path.startswith(('docs/', 'changelog.d/', '.cas/')) for path in paths)
        if code and anchor not in contained and f'- {anchor}' in git('cherry', 'HEAD', anchor).splitlines():
            included.add(task)
            continue
        if code and task not in deferred:
            missing.append(f'{task}: late delivery {anchor} on declared epic {target}; merge it or add a reasoned deferred_tasks entry')
    if missing:
        raise ChangelogError('release omits declared constituent deliveries:\n' + '\n'.join(sorted(missing)))
    return {'checked_sha': git('rev-parse', 'HEAD'), 'inventory_base': git('rev-parse', floor),
            'epic_refs': refs, 'deferred_tasks': deferred, 'included_task_ids': sorted(included)}


def main(argv):
    try:
        if len(argv) == 5 and argv[0] == 'promote':
            path = Path(argv[1])
            fragments = fragment_paths(path)
            path.write_text(promote(path.read_text(), argv[2], argv[3], argv[4],
                                    [fragment.read_text() for fragment in fragments]))
            for fragment in fragments:
                fragment.unlink()
        elif len(argv) == 5 and argv[0] == 'rebase':
            Path(argv[4]).write_text(
                rebase(Path(argv[1]).read_text(), Path(argv[2]).read_text(), argv[3]))
        elif argv and argv[0] == 'verify-deliveries':
            parser = argparse.ArgumentParser()
            parser.add_argument('base_ref')
            parser.add_argument('--notes')
            parser.add_argument('--report', type=Path)
            args = parser.parse_args(argv[1:])
            report = verify_deliveries(args.base_ref, args.notes)
            if args.report:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                temporary = args.report.with_suffix('.tmp')
                temporary.write_text(json.dumps(report, indent=2) + '\n')
                temporary.replace(args.report)
            print('task_ids=' + json.dumps(report['included_task_ids']))
        elif len(argv) == 3 and argv[0] == 'section':
            sys.stdout.write(section(Path(argv[1]).read_text(), argv[2]))
        else:
            sys.stderr.write(__doc__)
            return 2
    except (ChangelogError, ValueError, OSError) as error:
        print(f'error: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
