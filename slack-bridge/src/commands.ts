/**
 * Slack command detection for session lifecycle.
 *
 * Parses user messages for lifecycle keywords like "shut down", "reset",
 * "status", etc. and routes them to the session manager.
 */

import type { SessionManager } from "./session-manager.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type CommandType = "shutdown" | "reset" | "status" | null;

interface CommandResult {
  /** Reply text to post in the Slack thread */
  reply: string;
  /** Whether this was a recognized command (prevents message forwarding) */
  handled: boolean;
}

// ---------------------------------------------------------------------------
// Command detection
// ---------------------------------------------------------------------------

/** Patterns for detecting lifecycle commands in message text. */
const COMMAND_PATTERNS: Array<{ type: CommandType; patterns: RegExp[] }> = [
  {
    type: "shutdown",
    patterns: [
      /^shut\s*down$/i,
      /^stop$/i,
      /^done$/i,
      /^kill\s*session$/i,
      /^end\s*session$/i,
    ],
  },
  {
    type: "reset",
    patterns: [
      /^reset$/i,
      /^start\s*fresh$/i,
      /^new\s*session$/i,
      /^restart$/i,
    ],
  },
  {
    type: "status",
    patterns: [
      /^status$/i,
      /^sessions?$/i,
      /^what'?s?\s*running$/i,
    ],
  },
];

/**
 * Detect if a message is a lifecycle command.
 * Returns the command type or null if not a command.
 */
export function detectCommand(text: string): CommandType {
  const trimmed = text.trim();
  for (const { type, patterns } of COMMAND_PATTERNS) {
    for (const pattern of patterns) {
      if (pattern.test(trimmed)) {
        return type;
      }
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Command execution
// ---------------------------------------------------------------------------

/**
 * Execute a detected command against the session manager.
 */
async function executeCommand(
  commandType: CommandType,
  sessionManager: SessionManager,
  projectDir: string,
): Promise<CommandResult> {
  if (!commandType) {
    return { reply: "", handled: false };
  }

  switch (commandType) {
    case "shutdown": {
      const entry = sessionManager.get(projectDir);
      if (!entry) {
        return { reply: "No running session for this project.", handled: true };
      }
      const killed = await sessionManager.killSession(projectDir);
      if (killed) {
        return {
          reply: `Session \`${entry.name}\` shut down.`,
          handled: true,
        };
      }
      return { reply: "Failed to shut down session.", handled: true };
    }

    case "reset": {
      const existing = sessionManager.get(projectDir);
      const existingName = existing?.name;

      const result = await sessionManager.resetSession(projectDir);
      if (result) {
        const msg = existingName
          ? `Killed \`${existingName}\` and started new session \`${result.name}\`.`
          : `Started new session \`${result.name}\`.`;
        return { reply: msg, handled: true };
      }
      return { reply: "Failed to reset session.", handled: true };
    }

    case "status": {
      const sessions = sessionManager.list();
      if (sessions.length === 0) {
        return { reply: "No running sessions.", handled: true };
      }

      const lines = sessions.map((s) => {
        const idleMs = Date.now() - s.last_activity;
        const idleMin = Math.floor(idleMs / 60_000);
        const dir = s.project_dir.split("/").pop() ?? s.project_dir;
        return `• \`${s.name}\` — ${dir} (idle ${idleMin}m)`;
      });

      return {
        reply: `Running sessions:\n${lines.join("\n")}`,
        handled: true,
      };
    }

    default:
      return { reply: "", handled: false };
  }
}
