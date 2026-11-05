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

  release_changelog.py verify-deliveries <base-ref>
      Refuse fragment/commit-subject claims whose delivery is absent from HEAD.
"""
import re
import sys
import json
import os
import sqlite3
import subprocess
import tempfile
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


def verify_deliveries(base_ref):
    """Check release claims against the selected clone's recorded deliveries."""
    def git(*args, **kwargs):
        return subprocess.run(['git', *args], capture_output=True, **kwargs)

    def output(*args):
        result = git(*args)
        if result.returncode:
            raise ChangelogError(result.stderr.decode(errors='replace').strip())
        return result.stdout.decode().strip()

    paths = [str(path) for path in fragment_paths('CHANGELOG.md')]
    # A previous prepare consumed the fragments. Keep checking its claims on
    # reruns/re-bumps, rather than silently trusting the promoted prose.
    for commit in output('log', f'{base_ref}..HEAD', '--format=%H',
                         '--grep=^release: v[0-9]').splitlines():
        paths += output('diff', '--name-only', '--diff-filter=D', f'{commit}^',
                        commit, '--', 'changelog.d/').splitlines()
    tasks = {Path(path).stem for path in paths
             if re.fullmatch(r'cas-[0-9a-f]+\.md', Path(path).name)}
    # A bad rebase can drop the fix and its fragment together, leaving the
    # task's red/test commit and release prose behind.
    tasks.update(re.findall(r'\bcas-[0-9a-f]+\b',
                            output('log', f'{base_ref}..HEAD', '--format=%s')))
    if not tasks:
        return
    # Worktrees share their clone's store. Never use an inherited CAS_ROOT
    # from the factory that launched a release in a different fixture/clone.
    common = Path(output('rev-parse', '--path-format=absolute', '--git-common-dir'))
    database = common.parent / '.cas' / 'cas.db'
    try:
        with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as db:
            for task in sorted(tasks):
                row = db.execute('SELECT deliverables FROM tasks WHERE id = ?', (task,)).fetchone()
                delivery = json.loads(row[0]) if row and row[0] else {}
                if isinstance(delivery, str):
                    delivery = json.loads(delivery)
                history = delivery.get('historical_factory_branch_anchors') or []
                anchor = delivery.get('factory_branch_anchor') or (history[-1] if history else None)
                if not anchor:
                    raise ChangelogError(f'{task}: no recorded delivery commit for release claim')
                missing = f'{task}: delivery commit {anchor} is missing from HEAD'
                if not re.fullmatch(r'[0-9a-f]{40}', anchor) or git('cat-file', '-e', f'{anchor}^{{commit}}').returncode:
                    raise ChangelogError(missing)
                if git('merge-base', '--is-ancestor', anchor, 'HEAD').returncode == 0:
                    continue
                start = (delivery.get('factory_branch_start') or {}).get('base_sha')
                if not start or git('merge-base', '--is-ancestor', start, anchor).returncode:
                    raise ChangelogError(missing + '; no resolvable delivery base for patch comparison')
                delta = git('diff', '--binary', start, anchor, '--', '.',
                            ':(exclude)changelog.d/', ':(exclude)hub-web/dist/', ':(exclude)Cargo.lock')
                if delta.returncode:
                    raise ChangelogError(missing + '; cannot read its delivery patch')
                patch = delta.stdout
                # A cherry-pick/squash may change the commit identity. Check
                # the non-generated delivery delta against HEAD, not just a
                # final docs commit. Fragments, bundles and lockfiles are
                # promoted/regenerated by the release process.
                with tempfile.TemporaryDirectory(prefix='release-delivery-') as scratch:
                    env = dict(os.environ, GIT_INDEX_FILE=str(Path(scratch) / 'index'))
                    if git('read-tree', 'HEAD', env=env).returncode:
                        raise ChangelogError(missing + '; cannot read HEAD for patch comparison')
                    if patch and git('apply', '--cached', '--reverse', '--check', env=env, input=patch).returncode:
                        raise ChangelogError(missing + '; its delivery patch is absent too')
    except (sqlite3.Error, ValueError, AttributeError, TypeError) as error:
        raise ChangelogError(f'cannot verify release deliveries in {database}: {error}') from error


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
        elif len(argv) == 2 and argv[0] == 'verify-deliveries':
            verify_deliveries(argv[1])
        elif len(argv) == 3 and argv[0] == 'section':
            sys.stdout.write(section(Path(argv[1]).read_text(), argv[2]))
        else:
            sys.stderr.write(__doc__)
            return 2
    except ChangelogError as error:
        print(f'error: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
