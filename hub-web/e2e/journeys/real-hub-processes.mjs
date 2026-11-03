// Darwin has no /proc. Inspect only PID relationships and start fingerprints;
// never collect another process's arguments or environment to find ownership.
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);
async function processTable(args) {
  try {
    const { stdout } = await run("/bin/ps", [...args, "-o", "pid=,ppid=,lstart=,stat="], { env: { LC_ALL: "C" } });
    return stdout.trim().split("\n").flatMap(line => {
      const match = line.trim().match(/^(\d+)\s+(\d+)\s+(.+?)\s+(\S+)$/);
      return match && !match[4].includes("Z")
        ? [{ pid: Number(match[1]), parent: Number(match[2]), starttime: match[3] }] : [];
    });
  } catch (error) {
    if (error.code === 1 && !error.stdout.trim()) return [];
    throw error;
  }
}

export async function darwinProcessIdentity(pid) {
  return (await processTable(["-p", String(pid)]))[0];
}

export async function darwinOwnedProcesses(root, remembered) {
  const table = await processTable(["-ax"]);
  const owned = new Set(table.some(row => row.pid === root?.pid && row.starttime === root.starttime)
    ? [root.pid] : []);
  // Carry previously captured identities across reparenting, without admitting
  // a recycled PID. The root is the Node ChildProcess owned by this fixture.
  for (const row of table)
    if (remembered.get(row.pid)?.starttime === row.starttime) owned.add(row.pid);
  let added;
  do {
    added = false;
    for (const row of table) {
      if (owned.has(row.parent) && !owned.has(row.pid)) {
        owned.add(row.pid);
        added = true;
      }
    }
  } while (added);
  const rows = table.filter(row => row.pid !== root?.pid && owned.has(row.pid));
  for (const row of rows) remembered.set(row.pid, row);
  return rows;
}
