/**
 * Per-user daemon: Runs as a specific Linux user (e.g., cas-bridge@daniel).
 *
 * Listens on a Unix socket for forwarded Slack messages from the router,
 * injects them into the appropriate CAS factory session via `cas serve` HTTP API,
 * and streams responses back to the originating Slack thread via SSE.
 *
 * Reads CAS credentials from ~/.config/cas/env.
 */

import { createServer, type Server, type Socket } from "node:net";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { createHash, randomBytes } from "node:crypto";
import { dirname, resolve } from "node:path";
import { spawn as spawnChild } from "node:child_process";
import type { DaemonMessage } from "./router.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface DaemonConfig {
  /** Unix socket path this daemon listens on */
  socket_path: string;
  /** Linux username this daemon runs as */
  username: string;
  /** cas serve base URL (e.g., http://127.0.0.1:18999) */
  cas_serve_url: string;
  /** Bearer token for cas serve auth (empty if --no-auth) */
  cas_serve_token: string;
  /** Slack bot token for posting replies */
  slack_bot_token: string;
}

// ---------------------------------------------------------------------------
// Env loading
// ---------------------------------------------------------------------------

/**
 * Load daemon environment from ~/.config/cas/env.
 * Format: KEY=VALUE lines, # comments, empty lines ignored.
 */
export function loadDaemonEnv(
  envPath?: string,
): Record<string, string> {
  const path =
    envPath ??
    process.env.CAS_BRIDGE_ENV ??
    resolve(process.env.HOME ?? "/tmp", ".config/cas/env");

  const vars: Record<string, string> = {};
  if (!existsSync(path)) {
    console.warn(`Env file not found at ${path}`);
    return vars;
  }

  const lines = readFileSync(path, "utf-8").split("\n");
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eq = trimmed.indexOf("=");
    if (eq < 1) continue;
    const key = trimmed.slice(0, eq).trim();
    const val = trimmed.slice(eq + 1).trim();
    vars[key] = val;
  }
  return vars;
}

// ---------------------------------------------------------------------------
// CAS serve client
// ---------------------------------------------------------------------------

/**
 * Execute a Claude Code command directly in the project directory.
 *
 * Spawns `claude -p "message" --dangerously-skip-permissions` as a child
 * process and captures its output. This bypasses the factory/PTY pipeline
 * which has timing issues with Claude Code's TUI initialization.
 */
export interface ClaudeRunResult {
  code: number | null;
  stdout: string;
  stderr: string;
  spawnError?: string;
}

export type ClaudeRunner = (
  args: string[],
  cwd: string,
) => Promise<ClaudeRunResult>;

export interface MessageInjectorOptions {
  runner?: ClaudeRunner;
  sessionStatePath?: string;
}

type InjectionResult = {
  ok: boolean;
  session?: string;
  message_id?: number;
  response?: string;
  error?: string;
};

function threadScopeKey(msg: DaemonMessage): string {
  return JSON.stringify([msg.channel, msg.thread_ts, msg.project]);
}

/**
 * Convert the full Slack thread scope to a deterministic UUID-like session ID.
 * Claude Code requires a valid UUID format for --session-id.
 */
function threadScopeToSessionId(scopeKey: string): string {
  const hex = createHash("sha256").update(scopeKey).digest("hex").slice(0, 32);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

class ThreadSessionStore {
  private readonly sessions = new Map<string, string>();
  private loadError?: string;

  constructor(private readonly path: string) {
    if (!existsSync(path)) return;

    try {
      const stored = JSON.parse(readFileSync(path, "utf-8")) as unknown;
      if (!stored || typeof stored !== "object" || Array.isArray(stored)) {
        throw new Error("expected an object mapping thread scopes to session IDs");
      }
      for (const [scopeKey, sessionId] of Object.entries(stored)) {
        if (typeof sessionId !== "string") {
          throw new Error(`session ID for ${scopeKey} is not a string`);
        }
        this.sessions.set(scopeKey, sessionId);
      }
    } catch (err) {
      this.sessions.clear();
      this.loadError = err instanceof Error ? err.message : String(err);
    }
  }

  error(): string | undefined {
    return this.loadError;
  }

  get(scopeKey: string): string | undefined {
    return this.sessions.get(scopeKey);
  }

  record(scopeKey: string, sessionId: string): void {
    // The child established this session even if durable recovery cannot be saved.
    this.sessions.set(scopeKey, sessionId);
    const next = new Map(this.sessions);

    const directory = dirname(this.path);
    mkdirSync(directory, { recursive: true });
    const temporaryPath = `${this.path}.${process.pid}.${randomBytes(8).toString("hex")}.tmp`;
    try {
      writeFileSync(
        temporaryPath,
        `${JSON.stringify(Object.fromEntries(next), null, 2)}\n`,
        { encoding: "utf-8", flag: "wx", mode: 0o600 },
      );
      renameSync(temporaryPath, this.path);
    } finally {
      if (existsSync(temporaryPath)) unlinkSync(temporaryPath);
    }
  }
}

const runClaude: ClaudeRunner = (args, cwd) =>
  new Promise((resolve) => {
    let settled = false;
    const finish = (result: ClaudeRunResult) => {
      if (settled) return;
      settled = true;
      resolve(result);
    };

    const child = spawnChild("claude", args, {
      cwd,
      env: { ...process.env, HOME: process.env.HOME },
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 300_000,
    });

    let stdout = "";
    let stderr = "";

    child.stdout?.on("data", (chunk: Buffer) => {
      stdout += chunk.toString();
    });

    child.stderr?.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });

    child.on("close", (code) => finish({ code, stdout, stderr }));
    child.on("error", (err) =>
      finish({ code: null, stdout, stderr, spawnError: err.message }),
    );
  });

export function createMessageInjector(
  options: MessageInjectorOptions = {},
): (config: DaemonConfig, msg: DaemonMessage) => Promise<InjectionResult> {
  const runner = options.runner ?? runClaude;
  const statePath =
    options.sessionStatePath ??
    resolve(process.env.HOME ?? "/tmp", ".config/cas/slack-thread-sessions.json");
  const sessions = new ThreadSessionStore(statePath);
  const queues = new Map<string, Promise<InjectionResult>>();

  const execute = async (
    _config: DaemonConfig,
    msg: DaemonMessage,
    scopeKey: string,
  ): Promise<InjectionResult> => {
    const stateError = sessions.error();
    if (stateError) {
      return {
        ok: false,
        error: `Slack thread session state at ${statePath} could not be read safely: ${stateError}. Repair or remove the state file before retrying; no child was started.`,
      };
    }

    const storedSessionId = sessions.get(scopeKey);
    const sessionId = storedSessionId ?? threadScopeToSessionId(scopeKey);
    const isResume = storedSessionId !== undefined;

    const args = [
      "--dangerously-skip-permissions",
      "-p", msg.text,
      "--effort", "high",
      "--max-turns", "20",
    ];

    if (isResume) {
      // Resume existing session — adds new message to the conversation
      args.push("--resume", sessionId);
    } else {
      // New session — set session ID so we can resume later
      args.push("--session-id", sessionId);
    }

    console.log(`Spawning claude [${isResume ? "resume" : "new"}] in ${msg.project_dir}: ${msg.text.slice(0, 60)}`);

    let result: ClaudeRunResult;
    try {
      result = await runner(args, msg.project_dir);
    } catch (err) {
      return {
        ok: false,
        error: `spawn failed: ${err instanceof Error ? err.message : String(err)}`,
      };
    }

    if (result.spawnError) {
      return { ok: false, error: `spawn failed: ${result.spawnError}` };
    }
    if (result.code !== 0) {
      return {
        ok: false,
        error: `claude exited ${result.code}: ${result.stderr.slice(0, 200) || result.stdout.slice(0, 200) || "no output"}`,
      };
    }

    try {
      sessions.record(scopeKey, sessionId);
    } catch (err) {
      return {
        ok: false,
        error: `claude session succeeded and will resume in this daemon, but its Slack thread state could not be saved; restart recovery is uncertain: ${err instanceof Error ? err.message : String(err)}`,
      };
    }

    if (!result.stdout.trim()) {
      return { ok: false, error: "claude exited 0: no output" };
    }

    return {
      ok: true,
      session: "direct",
      message_id: Date.now(),
      response: result.stdout.trim(),
    };
  };

  return (config, msg) => {
    const scopeKey = threadScopeKey(msg);
    const previous = queues.get(scopeKey) ?? Promise.resolve({ ok: true });
    const current = previous
      .catch(() => ({ ok: false }))
      .then(() => execute(config, msg, scopeKey));
    const queued = current.finally(() => {
      if (queues.get(scopeKey) === queued) queues.delete(scopeKey);
    });
    queues.set(scopeKey, queued);
    return queued;
  };
}

const defaultMessageInjector = createMessageInjector();

export function injectMessage(
  config: DaemonConfig,
  msg: DaemonMessage,
): Promise<InjectionResult> {
  return defaultMessageInjector(config, msg);
}

// ---------------------------------------------------------------------------
// Unix socket server
// ---------------------------------------------------------------------------

export type MessageHandler = (msg: DaemonMessage) => Promise<void>;

/**
 * Start the Unix socket server that receives forwarded messages from the router.
 */
export function startSocketServer(
  socketPath: string,
  onMessage: MessageHandler,
): Server {
  // Clean up stale socket
  if (existsSync(socketPath)) {
    unlinkSync(socketPath);
  }

  // Ensure parent directory exists
  const dir = socketPath.slice(0, socketPath.lastIndexOf("/"));
  if (dir && !existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }

  const server = createServer((conn: Socket) => {
    let data = "";

    conn.on("data", (chunk) => {
      data += chunk.toString();
    });

    conn.on("end", () => {
      // Each connection sends one JSON message terminated by newline
      const lines = data.split("\n").filter((l) => l.trim());
      for (const line of lines) {
        try {
          const msg = JSON.parse(line) as DaemonMessage;
          onMessage(msg).catch((err) => {
            console.error(`Handler error: ${err}`);
          });
        } catch (err) {
          console.error(`Invalid JSON from router: ${err}`);
        }
      }
    });

    conn.on("error", (err) => {
      console.error(`Socket connection error: ${err.message}`);
    });
  });

  server.listen(socketPath, () => {
    console.log(`Daemon listening on ${socketPath}`);
  });

  // Make socket world-writable so the unprivileged router can connect
  server.on("listening", () => {
    chmodSync(socketPath, 0o777);
  });

  return server;
}
