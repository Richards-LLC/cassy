// Owned-process discovery for real-hub fixtures. Darwin has no /proc: there,
// inspect only PID relationships and start fingerprints, never another
// process's arguments or environment.
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

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

// Linux: this fixture's unique HOME marks its processes, including PTY
// children that lead their own session or were reparented when the reaper exited.
export async function linuxProcessIdentity(pid) {
  try {
    const stat = await readFile(`/proc/${pid}/stat`, "utf8");
    const fields = stat.slice(stat.lastIndexOf(") ") + 2).split(/\s+/);
    return fields[0] === "Z" ? undefined
      : { pid: Number(pid), parent: Number(fields[1]), starttime: fields[19] };
  } catch (error) {
    if (["ENOENT", "ESRCH"].includes(error.code)) return;
    throw error;
  }
}

async function linuxOwnedProcesses(home) {
  const expected = `HOME=${home}`;
  const processes = await Promise.all((await readdir("/proc"))
    .filter(pid => /^\d+$/.test(pid)).map(async pid => {
      try {
        const identity = await linuxProcessIdentity(pid);
        if (!identity) return;
        const env = await readFile(`/proc/${pid}/environ`);
        if (!env.toString().split("\0").includes(expected)) return;
        if ((await linuxProcessIdentity(pid))?.starttime === identity.starttime) return identity;
      } catch (error) {
        if (["ENOENT", "ESRCH", "EACCES"].includes(error.code)) return;
        throw error;
      }
    }));
  return processes.filter(Boolean);
}

async function signalOwned(identity, signal, identify) {
  if ((await identify(identity.pid))?.starttime !== identity.starttime) return;
  try { process.kill(identity.pid, signal); }
  catch (error) { if (error.code !== "ESRCH") throw error; }
}

// Stop every captured owned process, and return only once none remains.
// With a leader, its children (the daemon) stop first so the leader (the
// reaper) can wait for them and finish its exit receipt.
export async function stopOwnedProcesses(list, identify, leader) {
  let processes = await list();
  const exited = async () => {
    const deadline = performance.now() + 3000;
    do {
      processes = await list();
      if (!processes.length) return true;
      await new Promise(ok => setTimeout(ok, 50));
    } while (performance.now() < deadline);
    return false;
  };
  if (leader !== undefined) {
    for (const child of processes.filter(p => p.parent === leader))
      await signalOwned(child, "SIGTERM", identify);
    if (await exited()) return;
  }
  // Each signal is bounded to a PID that is still owned with the same start
  // fingerprint, including a provider detached into its own process group.
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    for (const child of processes) await signalOwned(child, signal, identify);
    if (await exited()) return;
  }
  throw Error("owned launch processes did not exit; fixture retained");
}

// Linux cleanup before the fixture HOME is removed. A daemon writes lifecycle
// records into HOME while it tears down, so every HOME-owned process must be
// gone first. The launch shim records the hub-started reaper in launched-pid
// only after it starts, so a launch caught mid-start has no record: sweep then.
export async function stopLinuxFixtureProcesses(home) {
  const launched = join(home, "launched-pid");
  let leader;
  if (existsSync(launched)) {
    const owned = JSON.parse(await readFile(launched, "utf8"));
    if (!Number.isSafeInteger(owned.pid) || owned.pid <= 1)
      throw Error("invalid owned launch PID; fixture retained");
    const identity = await linuxProcessIdentity(owned.pid);
    if (identity && identity.starttime !== owned.starttime)
      throw Error("owned launch PID was reused; fixture retained");
    leader = owned.pid;
  }
  await stopOwnedProcesses(() => linuxOwnedProcesses(home), linuxProcessIdentity, leader);
}
