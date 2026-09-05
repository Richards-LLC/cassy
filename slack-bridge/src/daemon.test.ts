import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  createMessageInjector,
  type ClaudeRunResult,
  type ClaudeRunner,
  type DaemonConfig,
} from "./daemon.js";
import type { DaemonMessage } from "./router.js";

const config: DaemonConfig = {
  socket_path: "/tmp/cas-bridge-test.sock",
  username: "bridge-test",
  cas_serve_url: "http://127.0.0.1:18999",
  cas_serve_token: "",
  slack_bot_token: "",
};

const baseMessage: DaemonMessage = {
  channel: "C012345",
  thread_ts: "1757020000.123451",
  slack_user: "U012345",
  text: "hello",
  project_dir: "/work/project-a",
  project: "project-a",
};

/** The deterministic session ID `baseMessage` resolves to. */
const threadSessionId = "7265c816-1515-b05e-2028-865a7a7730d3";

const temporaryDirectories: string[] = [];

function statePath(): string {
  const directory = mkdtempSync(join(tmpdir(), "cas-slack-daemon-test-"));
  temporaryDirectories.push(directory);
  return join(directory, "thread-sessions.json");
}

function sessionId(args: string[]): string {
  const index = args.indexOf("--session-id");
  expect(index).toBeGreaterThanOrEqual(0);
  return args[index + 1];
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe("Slack thread message injection", () => {
  it("assigns distinct sessions to near-identical timestamps and different scopes", async () => {
    const calls: string[][] = [];
    const runner: ClaudeRunner = async (args) => {
      calls.push(args);
      return { code: 0, stdout: "ok", stderr: "" };
    };
    const inject = createMessageInjector({ runner, sessionStatePath: statePath() });

    await inject(config, baseMessage);
    await inject(config, { ...baseMessage, thread_ts: "1757020000.123459" });
    await inject(config, { ...baseMessage, channel: "C987654" });
    await inject(config, { ...baseMessage, project: "project-b" });

    const ids = calls.map(sessionId);
    expect(new Set(ids).size).toBe(4);
    expect(ids).toEqual([
      "7265c816-1515-b05e-2028-865a7a7730d3",
      "ea3b785c-9b6a-35b1-33ef-0b918c17ffe4",
      "a8434afe-40fc-5e44-b6ba-9a6e794a8ea3",
      "d5509c6c-8edb-ccb0-5a57-bad19499b4ab",
    ]);
  });

  it("records establishment only after success and resumes after restart", async () => {
    const path = statePath();
    const calls: string[][] = [];
    const results: ClaudeRunResult[] = [
      { code: 1, stdout: "", stderr: "startup failed" },
      { code: 0, stdout: "started", stderr: "" },
      { code: 0, stdout: "resumed", stderr: "" },
      { code: 0, stdout: "resumed after restart", stderr: "" },
    ];
    const runner: ClaudeRunner = async (args) => {
      calls.push(args);
      return results.shift()!;
    };

    const firstDaemon = createMessageInjector({ runner, sessionStatePath: path });
    expect((await firstDaemon(config, baseMessage)).ok).toBe(false);
    expect((await firstDaemon(config, baseMessage)).ok).toBe(true);
    expect((await firstDaemon(config, baseMessage)).ok).toBe(true);

    const restartedDaemon = createMessageInjector({ runner, sessionStatePath: path });
    expect((await restartedDaemon(config, baseMessage)).ok).toBe(true);

    expect(calls.slice(0, 2).every((args) => args.includes("--session-id"))).toBe(true);
    expect(calls.slice(2).every((args) => args.includes("--resume"))).toBe(true);
    expect(calls.map((args) => args.at(-1))).toEqual([
      "7265c816-1515-b05e-2028-865a7a7730d3",
      "7265c816-1515-b05e-2028-865a7a7730d3",
      "7265c816-1515-b05e-2028-865a7a7730d3",
      "7265c816-1515-b05e-2028-865a7a7730d3",
    ]);
  });

  it("fails closed without spawning when durable session state is corrupt", async () => {
    const path = statePath();
    writeFileSync(path, "not json");
    let calls = 0;
    const inject = createMessageInjector({
      runner: async () => {
        calls += 1;
        return { code: 0, stdout: "unexpected", stderr: "" };
      },
      sessionStatePath: path,
    });

    const result = await inject(config, baseMessage);
    expect(result).toMatchObject({ ok: false });
    expect(result.error).toContain("could not be read");
    expect(result.error).toContain(path);
    expect(calls).toBe(0);
  });

  it("recovers in the same injector once a corrupt state file is repaired", async () => {
    const path = statePath();
    writeFileSync(path, "not json");
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return { code: 0, stdout: "ok", stderr: "" };
      },
      sessionStatePath: path,
    });

    const rejected = await inject(config, baseMessage);
    expect(rejected.ok).toBe(false);
    expect(rejected.error).toContain("no restart needed");
    expect(calls).toHaveLength(0);

    // Exactly what the error tells the operator to do — repair the file, then
    // send the message again. The same injector must accept it.
    writeFileSync(path, JSON.stringify({}));
    const repaired = await inject(config, baseMessage);
    expect(repaired.ok).toBe(true);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toContain("--session-id");

    // A recorded session survives a later corruption + repair cycle.
    writeFileSync(path, "not json again");
    expect((await inject(config, baseMessage)).ok).toBe(false);
    writeFileSync(path, JSON.stringify({}));
    expect((await inject(config, baseMessage)).ok).toBe(true);
    expect(calls).toHaveLength(2);
    expect(calls[1]).toContain("--resume");
  });

  it("recovers in the same injector once a corrupt state file is removed", async () => {
    const path = statePath();
    writeFileSync(path, "[]");
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return { code: 0, stdout: "ok", stderr: "" };
      },
      sessionStatePath: path,
    });

    expect((await inject(config, baseMessage)).ok).toBe(false);
    rmSync(path);
    expect((await inject(config, baseMessage)).ok).toBe(true);
    expect(calls.map((args) => args.at(-2))).toEqual(["--session-id"]);
  });

  it("resumes instead of wedging when the deterministic session ID already exists", async () => {
    const path = statePath();
    const calls: string[][] = [];
    // The exact refusal Claude Code emits for an existing ID (2.1.261), on the
    // stderr channel its session-ID diagnostics were measured to use.
    const inUse: ClaudeRunResult = {
      code: 1,
      stdout: "",
      stderr: `Error: Session ID ${threadSessionId} is already in use.`,
    };
    const results: ClaudeRunResult[] = [
      // First message: the child established the session, then failed late, so
      // nothing was recorded here.
      { code: 1, stdout: "", stderr: "Error: reached max turns" },
      inUse,
      { code: 0, stdout: "recovered", stderr: "" },
      { code: 0, stdout: "resumed", stderr: "" },
    ];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return results.shift()!;
      },
      sessionStatePath: path,
    });

    expect((await inject(config, baseMessage)).ok).toBe(false);

    const recovered = await inject(config, baseMessage);
    expect(recovered).toMatchObject({ ok: true, response: "recovered" });

    // The recovery is durable: the next message resumes without a failed probe.
    expect((await inject(config, baseMessage)).ok).toBe(true);

    expect(calls.map((args) => args.at(-2))).toEqual([
      "--session-id",
      "--session-id",
      "--resume",
      "--resume",
    ]);
    expect(new Set(calls.map((args) => args.at(-1))).size).toBe(1);
    expect(calls.every((args) => args.at(-1) === threadSessionId)).toBe(true);
  });

  it("retries the refused session exactly once and reports a failing retry", async () => {
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        // Both attempts refuse: the new-session attempt because the ID exists,
        // the resume because the session is unusable. One retry, then stop.
        return args.includes("--session-id")
          ? {
              code: 1,
              stdout: "",
              stderr: `Error: Session ID ${threadSessionId} is already in use.`,
            }
          : { code: 1, stdout: "", stderr: "Error: session is corrupt" };
      },
      sessionStatePath: statePath(),
    });

    const result = await inject(config, baseMessage);
    expect(result.ok).toBe(false);
    expect(result.error).toContain("session is corrupt");
    expect(calls.map((args) => args.at(-2))).toEqual(["--session-id", "--resume"]);
  });

  it("does not retry an unrelated new-session failure", async () => {
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return { code: 1, stdout: "", stderr: "Error: reached max turns" };
      },
      sessionStatePath: statePath(),
    });

    const result = await inject(config, baseMessage);
    expect(result.ok).toBe(false);
    expect(result.error).toContain("reached max turns");
    expect(calls).toHaveLength(1);
  });

  it("does not treat model prose about session IDs as the CLI refusal", async () => {
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return {
          code: 1,
          // Answer text is not a diagnostic. Retrying here would replay the
          // message into a session the child never established.
          stdout: "The documentation says a session ID is already in use.",
          stderr: "Error: reached max turns",
        };
      },
      sessionStatePath: statePath(),
    });

    const result = await inject(config, baseMessage);
    expect(result.ok).toBe(false);
    expect(result.error).toContain("reached max turns");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toContain("--session-id");
  });

  it("does not retry a refusal that names a different session ID", async () => {
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return {
          code: 1,
          stdout: "",
          stderr: "Error: Session ID 00000000-0000-0000-0000-000000000000 is already in use.",
        };
      },
      sessionStatePath: statePath(),
    });

    const result = await inject(config, baseMessage);
    expect(result.ok).toBe(false);
    expect(calls).toHaveLength(1);
  });

  it("does not retry the refusal text arriving on stdout instead of the error channel", async () => {
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return {
          code: 1,
          stdout: `Error: Session ID ${threadSessionId} is already in use.`,
          stderr: "Error: reached max turns",
        };
      },
      sessionStatePath: statePath(),
    });

    expect((await inject(config, baseMessage)).ok).toBe(false);
    expect(calls).toHaveLength(1);
  });

  it("remembers a successful child in memory when durable persistence fails", async () => {
    const directory = mkdtempSync(join(tmpdir(), "cas-slack-daemon-test-"));
    temporaryDirectories.push(directory);
    const blockedParent = join(directory, "not-a-directory");
    writeFileSync(blockedParent, "file");
    const calls: string[][] = [];
    const inject = createMessageInjector({
      runner: async (args) => {
        calls.push(args);
        return { code: 0, stdout: "ok", stderr: "" };
      },
      sessionStatePath: join(blockedParent, "thread-sessions.json"),
    });

    expect((await inject(config, baseMessage)).ok).toBe(false);
    expect((await inject(config, baseMessage)).ok).toBe(false);
    expect(calls[0]).toContain("--session-id");
    expect(calls[1]).toContain("--resume");
  });

  it("serializes messages for the same scoped thread", async () => {
    const releases: Array<(result: ClaudeRunResult) => void> = [];
    let active = 0;
    let maximumActive = 0;
    const runner: ClaudeRunner = async () => {
      active += 1;
      maximumActive = Math.max(maximumActive, active);
      const result = await new Promise<ClaudeRunResult>((resolve) => releases.push(resolve));
      active -= 1;
      return result;
    };
    const inject = createMessageInjector({ runner, sessionStatePath: statePath() });

    const first = inject(config, baseMessage);
    const second = inject(config, { ...baseMessage, text: "follow-up" });
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(releases).toHaveLength(1);

    releases.shift()!({ code: 0, stdout: "first", stderr: "" });
    await first;
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(releases).toHaveLength(1);

    releases.shift()!({ code: 0, stdout: "second", stderr: "" });
    await second;
    expect(maximumActive).toBe(1);
  });
});
