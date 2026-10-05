#!/usr/bin/env python3
"""Load the publisher's real zigbuild Cargo config without compiling a crate.

zigbuild metadata/help bypass cargo-config2. Its build path loads that parser
before spawning CARGO; replace only that final child with a refusing delegate.
The sentinel proves the parser and linker preparation reached the build child.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main():
    worktree = Path(sys.argv[1] if len(sys.argv) > 1 else '.').resolve()
    with tempfile.TemporaryDirectory(prefix='cas-zigbuild-config-') as temporary:
        scratch = Path(temporary)
        delegate = scratch / 'cargo-no-build'
        sentinel = scratch / 'parsed'
        delegate.write_text('''#!/usr/bin/env python3
import os, sys
from pathlib import Path
expected = ['build', '-p', 'cas', '--release', '--target', 'x86_64-unknown-linux-gnu', '--locked']
if sorted(sys.argv[1:]) != sorted(expected):
    print('zigbuild config probe: unexpected delegate command; refusing execution', file=sys.stderr)
    sys.exit(2)
Path(os.environ['CAS_ZIGBUILD_CONFIG_SENTINEL']).write_text('parsed')
''')
        delegate.chmod(0o700)
        environment = dict(os.environ, CARGO=str(delegate),
                           CAS_ZIGBUILD_CONFIG_SENTINEL=str(sentinel))
        # Match release.sh's Linux publisher target and flags, and keep its cwd
        # and Cargo home/config environment. No Cargo build process can run.
        try:
            result = subprocess.run(
                ['cargo-zigbuild', 'zigbuild', '-p', 'cas', '--release',
                 '--target', 'x86_64-unknown-linux-gnu', '--locked'],
                cwd=worktree, env=environment, timeout=60, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f'zigbuild config probe failed: {error}', file=sys.stderr)
            return 1
        if result.returncode:
            print('zigbuild rejected publish toolchain/config; no build ran', file=sys.stderr)
            return 1
        if not sentinel.is_file():
            print('zigbuild config probe did not reach the no-build delegate', file=sys.stderr)
            return 1
    print('zigbuild publish config parsed; no build ran')
    return 0


if __name__ == '__main__':
    sys.exit(main())
