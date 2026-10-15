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

  release_changelog.py rebase <ours-file> <base-file> <merge-base-version> <out-file>
      Resolve a CHANGELOG merge conflict with the base branch: the result is
      the base file with this branch's unreleased entries (as `promote` reads
      them, relative to <merge-base-version>) under its [Unreleased] heading.
      Blocks the base already carries are dropped, so entries that reached
      this branch through an earlier merge of the base are not duplicated.

  release_changelog.py section <file> <version>
      Print the body of the <version> section.
"""
import re
import sys
from pathlib import Path

HEADING = re.compile(r'^## \[(?P<version>[^\]]+)\]')
UNRELEASED = '## [Unreleased]'


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


def promote(text, version, date, base_version):
    if version_key(version) is None:
        raise ChangelogError(f'release version is not x.y.z: {version!r}')
    lines = text.splitlines()
    entries, start, end = region_entries(lines, base_version)
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


def main(argv):
    try:
        if len(argv) == 5 and argv[0] == 'promote':
            path = Path(argv[1])
            path.write_text(promote(path.read_text(), argv[2], argv[3], argv[4]))
        elif len(argv) == 5 and argv[0] == 'rebase':
            Path(argv[4]).write_text(
                rebase(Path(argv[1]).read_text(), Path(argv[2]).read_text(), argv[3]))
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
