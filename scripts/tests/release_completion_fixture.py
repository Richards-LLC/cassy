#!/usr/bin/env python3
"""Supply real Git, archive and executable proof to release-train stage fixtures.

External stages may simulate publication/update, but cannot bypass the production
completion gate. All paths here belong to the enclosing temporary test fixture.
"""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile

version, run_path, worktree_path, cas_path = sys.argv[1:]
run, worktree, cas = Path(run_path), Path(worktree_path), Path(cas_path)

def git(*args):
    return subprocess.run(["git", "-C", str(worktree), *args], check=True,
                          capture_output=True, text=True).stdout.strip()

landed = (run / "landed-main.sha").read_text().strip()
tag = f"v{version}"
git("tag", "-f", tag, landed)
git("push", "origin", f"{landed}:refs/heads/main", f"refs/tags/{tag}")
original = cas.read_text()
condition = '"$*" == --version'
if os.environ.get("COMPLETION_HOST_ENV_PROBE"):
    condition += ' && -z "${HOST_STUB_LOG:-}"'
# Preserve the discovery/assembly command while providing --version independently
# of test env variables, as the isolated clean-install probe must require.
cas.write_text(f'#!/usr/bin/env bash\nif [[ {condition} ]]; then\n'
               f'  echo "cas {version} ({landed[:7]} 2099-01-01)"; exit 0\nfi\n'
               + original)
cas.chmod(0o755)
binary = cas.read_bytes()
assets = []
for prefix, triple in (("LINUX", "x86_64-unknown-linux-gnu"), ("MACOS", "aarch64-apple-darwin")):
    name = f"cas-{triple}.tar.gz"
    with tarfile.open(run / name, "w:gz") as archive:
        member = tarfile.TarInfo("cas")
        member.size, member.mode = len(binary), 0o755
        archive.addfile(member, io.BytesIO(binary))
    sha = hashlib.sha256((run / name).read_bytes()).hexdigest()
    assets.append((prefix, name, sha))
(run / "release-workflow.json").write_text(json.dumps({"headSha": landed, "headBranch": tag,
                                                      "status": "completed", "conclusion": "success"}))
(run / "release-published.receipt").write_text(
    f"TAG={tag}\nPUBLISHED_AT=2099-01-01T00:00:00Z\n"
    + "".join(f"{prefix}_ASSET={name}\n{prefix}_SHA256={sha}\n" for prefix, name, sha in assets))
(run / "host-update.json").write_text(json.dumps({"status": "PASS", "version": version,
    "cas_version": version, "hub_version": version, "hub_running": True,
    "refresh_binary_version": version}))
(run / "fixture-publication.json").write_text(json.dumps({"isDraft": False,
    "publishedAt": "2099-01-01T00:00:00Z",
    "assets": [{"name": name, "digest": "sha256:" + sha} for _, name, sha in assets]}))
# The caller supplies a dedicated GitHub CLI fixture; never replace an installed
# tool. Its preexisting PR branches remain available to receipts-stage tests.
gh = Path(os.environ["CAS_RELEASE_TRAIN_GH"])
original_path = gh.with_name(gh.name + ".original")
if not original_path.exists():
    original_path.write_text(gh.read_text() if gh.exists() else "exit 65\n")
original = original_path.read_text()
gh.write_text('''#!/usr/bin/env python3
import pathlib, shutil, subprocess, sys
root = pathlib.Path(''' + repr(str(run)) + ''')
if sys.argv[1:3] == ['release', 'view']:
    print((root / 'fixture-publication.json').read_text())
elif sys.argv[1:3] == ['release', 'download']:
    name = sys.argv[sys.argv.index('--pattern') + 1]
    shutil.copyfile(root / name, pathlib.Path(sys.argv[sys.argv.index('--dir') + 1]) / name)
else:
    raise SystemExit(subprocess.call(['bash', '-c', ''' + repr(original) + ''', 'fixture-gh', *sys.argv[1:]]))
''')
gh.chmod(0o755)
