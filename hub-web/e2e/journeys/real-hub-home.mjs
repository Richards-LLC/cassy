// Disposable HOME allocation shared by the native fixture and its path-budget test.
import { mkdtemp, realpath, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export async function createRealHubHome({ platform = process.platform, temporaryDirectory = tmpdir() } = {}) {
  // Darwin's default /var/folders TMPDIR leaves too little sun_path space
  // for the daemon's longer GUI socket. Canonical /tmp is /private/tmp.
  const base = platform === 'darwin' ? '/tmp' : temporaryDirectory;
  const prefix = platform === 'darwin' ? 'cas-rh-' : 'cas-real-hub-';
  return mkdtemp(join(await realpath(base), prefix));
}

export async function createRealHubEnvironment(home, project) {
  // launch_path probes $SHELL -lc before the ambient PATH. A real login shell
  // loads /etc/profile (including macOS path_helper), which can put the host's
  // provider ahead of our inert one even with a disposable HOME. Keep this
  // boundary local to the fixture; product service PATH resolution stays real.
  const shell = join(home, "bin", "sh");
  await writeFile(shell, `#!/bin/sh
if [ "$1" = "-lc" ]; then
  shift
  exec /bin/sh -c "$@"
fi
exec /bin/sh "$@"
`, { mode: 0o700 });
  return {
    HOME: home,
    XDG_CONFIG_HOME: join(home, "config"),
    XDG_RUNTIME_DIR: join(home, "runtime"),
    CAS_ROOT: join(project, ".cas"),
    CAS_AGENT_NAME: process.env.CAS_AGENT_NAME ?? "real-hub-qa",
    PATH: `${join(home, "bin")}:/usr/bin:/bin`,
    SHELL: shell,
    TERM: "xterm-256color",
    LANG: "C.UTF-8",
    CAS_SKIP_FACTORY_TOOLING: "1",
    CAS_SUPPRESS_DUPLICATE_WARNING: "1",
    RUST_LOG: "cas=debug",
  };
}
