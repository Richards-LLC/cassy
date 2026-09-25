/**
 * Session lifecycle management for the Slack bridge.
 *
 * Tracks CAS factory sessions per user+project, handles lazy startup,
 * idle timeout, and cleanup of orphaned sessions.
 */

import type { DaemonConfig } from "./daemon.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SessionEntry {
  /** CAS session name */
  name: string;
  /** Project directory */
  project_dir: string;
  /** Last activity timestamp (ms since epoch) */
  last_activity: number;
  /** When the session was started (ms since epoch) */
  started_at: number;
}

export interface SessionManagerConfig {
  /** Idle timeout in milliseconds (default: 30 minutes) */
  idle_timeout_ms: number;
  /** Warning before shutdown in milliseconds (default: 5 minutes) */
  idle_warning_ms: number;
  /** Polling interval for idle check in milliseconds (default: 60 seconds) */
  idle_check_interval_ms: number;
}

export const DEFAULT_SESSION_CONFIG: SessionManagerConfig = {
  idle_timeout_ms: 30 * 60 * 1000, // 30 minutes
  idle_warning_ms: 5 * 60 * 1000,  // 5 minutes
  idle_check_interval_ms: 60 * 1000, // 1 minute
};

export type IdleCallback = (entry: SessionEntry, warning: boolean) => Promise<void>;

// ---------------------------------------------------------------------------
// CAS serve HTTP helpers
// ---------------------------------------------------------------------------

function headers(config: DaemonConfig): Record<string, string> {
  const h: Record<string, string> = { "Content-Type": "application/json" };
  if (config.cas_serve_token) {
    h["Authorization"] = `Bearer ${config.cas_serve_token}`;
  }
  return h;
}

/** List running sessions from cas serve, optionally filtering by project_dir. */
async function listSessions(
  config: DaemonConfig,
  projectDir?: string,
): Promise<Array<{ name: string; project_dir: string | null; is_running: boolean; can_attach: boolean }>> {
  const url = new URL(`${config.cas_serve_url}/v1/sessions`);
  if (projectDir) url.searchParams.set("project_dir", projectDir);
  url.searchParams.set("running_only", "true");

  const res = await fetch(url.toString(), { headers: headers(config) });
  if (!res.ok) return [];

  const data = (await res.json()) as {
    sessions: Array<{ name: string; project_dir: string | null; is_running: boolean; can_attach: boolean }>;
  };
  return data.sessions ?? [];
}

/** Kill a session via cas serve. */
async function killSession(
  config: DaemonConfig,
  sessionName: string,
): Promise<boolean> {
  const res = await fetch(
    `${config.cas_serve_url}/v1/sessions/${sessionName}/kill`,
    { method: "POST", headers: headers(config) },
  );
  return res.ok;
}

/** Start a factory session via cas serve. Returns session name or null. */
async function startSession(
  config: DaemonConfig,
  projectDir: string,
): Promise<{ name: string; reused: boolean } | null> {
  const res = await fetch(`${config.cas_serve_url}/v1/factory/start`, {
    method: "POST",
    headers: headers(config),
    body: JSON.stringify({
      project_dir: projectDir,
      reuse_existing: true,
      workers: 0,
    }),
  });

  if (!res.ok) return null;

  const data = (await res.json()) as {
    session: { name: string };
    started: boolean;
    reused_existing: boolean;
  };

  return {
    name: data.session.name,
    reused: data.reused_existing,
  };
}

// ---------------------------------------------------------------------------
// Session Manager
// ---------------------------------------------------------------------------

export class SessionManager {
  private sessions = new Map<string, SessionEntry>();
  private config: SessionManagerConfig;
  private daemonConfig: DaemonConfig;
  private idleTimer: ReturnType<typeof setInterval> | null = null;
  private onIdle: IdleCallback | null = null;
  private warned = new Set<string>();

  constructor(daemonConfig: DaemonConfig, config?: Partial<SessionManagerConfig>) {
    this.daemonConfig = daemonConfig;
    this.config = { ...DEFAULT_SESSION_CONFIG, ...config };
  }

  /** Key for the session map: project_dir */
  private key(projectDir: string): string {
    return projectDir;
  }

  /** Get a tracked session for a project, or null. */
  get(projectDir: string): SessionEntry | null {
    return this.sessions.get(this.key(projectDir)) ?? null;
  }

  /** Track a session. */
  track(entry: SessionEntry): void {
    this.sessions.set(this.key(entry.project_dir), entry);
    this.warned.delete(this.key(entry.project_dir));
  }

  /** Remove a tracked session. */
  untrack(projectDir: string): void {
    this.sessions.delete(this.key(projectDir));
    this.warned.delete(this.key(projectDir));
  }

  /** Update the last_activity timestamp for a session. */
  touch(projectDir: string): void {
    const entry = this.sessions.get(this.key(projectDir));
    if (entry) {
      entry.last_activity = Date.now();
      this.warned.delete(this.key(projectDir));
    }
  }

  /** List all tracked sessions. */
  list(): SessionEntry[] {
    return Array.from(this.sessions.values());
  }

  /** Number of tracked sessions. */
  get size(): number {
    return this.sessions.size;
  }

  /**
   * Ensure a session exists for the given project. Starts one if needed.
   * Returns the session name, or null if startup failed.
   */
  async ensureSession(projectDir: string): Promise<{ name: string; started: boolean } | null> {
    // Check local tracking first
    const existing = this.get(projectDir);
    if (existing) {
      this.touch(projectDir);
      return { name: existing.name, started: false };
    }

    // Try to start/reuse via cas serve
    const result = await startSession(this.daemonConfig, projectDir);
    if (!result) return null;

    const now = Date.now();
    this.track({
      name: result.name,
      project_dir: projectDir,
      last_activity: now,
      started_at: now,
    });

    return { name: result.name, started: !result.reused };
  }

  /**
   * Kill a session for a project and remove tracking.
   */
  async killSession(projectDir: string): Promise<boolean> {
    const entry = this.get(projectDir);
    if (!entry) return false;

    const killed = await killSession(this.daemonConfig, entry.name);
    this.untrack(projectDir);
    return killed;
  }

  /**
   * Kill and restart a session (reset).
   */
  async resetSession(projectDir: string): Promise<{ name: string } | null> {
    await this.killSession(projectDir);

    const result = await startSession(this.daemonConfig, projectDir);
    if (!result) return null;

    const now = Date.now();
    this.track({
      name: result.name,
      project_dir: projectDir,
      last_activity: now,
      started_at: now,
    });

    return { name: result.name };
  }

  /**
   * Start the idle timeout checker. Calls onIdle with (entry, warning=true)
   * when a session has been idle for (timeout - warning) ms, and
   * (entry, warning=false) when the full timeout expires (session will be killed).
   */
  startIdleChecker(onIdle: IdleCallback): void {
    this.onIdle = onIdle;
    this.idleTimer = setInterval(() => {
      this.checkIdle().catch((err) => {
        console.error(`Idle check error: ${err}`);
      });
    }, this.config.idle_check_interval_ms);
  }

  /** Stop the idle timeout checker. */
  stopIdleChecker(): void {
    if (this.idleTimer) {
      clearInterval(this.idleTimer);
      this.idleTimer = null;
    }
    this.onIdle = null;
  }

  /** Run one idle check cycle. Exported for testing. */
  async checkIdle(): Promise<void> {
    const now = Date.now();
    const warningThreshold = this.config.idle_timeout_ms - this.config.idle_warning_ms;

    for (const [key, entry] of this.sessions) {
      const idle = now - entry.last_activity;

      if (idle >= this.config.idle_timeout_ms) {
        // Full timeout — kill session
        if (this.onIdle) {
          await this.onIdle(entry, false);
        }
        await killSession(this.daemonConfig, entry.name);
        this.sessions.delete(key);
        this.warned.delete(key);
      } else if (idle >= warningThreshold && !this.warned.has(key)) {
        // Warning threshold — notify user
        if (this.onIdle) {
          await this.onIdle(entry, true);
        }
        this.warned.add(key);
      }
    }
  }

  /**
   * Startup cleanup: discover running CAS sessions and adopt or kill orphans.
   * Sessions that match tracked entries are adopted. Unknown sessions are killed.
   */
  async cleanupOrphans(adoptProjectDirs?: string[]): Promise<{ adopted: string[]; killed: string[] }> {
    const adopted: string[] = [];
    const killed: string[] = [];

    const sessions = await listSessions(this.daemonConfig);

    for (const session of sessions) {
      if (!session.is_running || !session.project_dir) continue;

      const projectDir = session.project_dir;
      const shouldAdopt = adoptProjectDirs
        ? adoptProjectDirs.includes(projectDir)
        : true;

      if (shouldAdopt) {
        const now = Date.now();
        this.track({
          name: session.name,
          project_dir: projectDir,
          last_activity: now,
          started_at: now,
        });
        adopted.push(session.name);
      } else {
        await killSession(this.daemonConfig, session.name);
        killed.push(session.name);
      }
    }

    return { adopted, killed };
  }

  /** Shutdown: stop idle checker and kill all tracked sessions. */
  async shutdown(): Promise<void> {
    this.stopIdleChecker();
    for (const entry of this.sessions.values()) {
      await killSession(this.daemonConfig, entry.name);
    }
    this.sessions.clear();
  }
}
