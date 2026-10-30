// Disposable HOME allocation shared by the native fixture and its path-budget test.
import { mkdtemp, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export async function createRealHubHome({ platform = process.platform, temporaryDirectory = tmpdir() } = {}) {
  // Darwin's default /var/folders TMPDIR leaves too little sun_path space
  // for the daemon's longer GUI socket. Canonical /tmp is /private/tmp.
  const base = platform === 'darwin' ? '/tmp' : temporaryDirectory;
  const prefix = platform === 'darwin' ? 'cas-rh-' : 'cas-real-hub-';
  return mkdtemp(join(await realpath(base), prefix));
}
