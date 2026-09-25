/**
 * Router: Unprivileged Slack event handler.
 *
 * Receives Slack message events via Bolt (Socket Mode), resolves the Slack user
 * to a Linux username via the allowlist, and forwards the message to the
 * per-user daemon via Unix socket.
 *
 * Runs as the `cas-bridge` system user with NO shell access and NO credentials.
 * The per-user daemon handles all CAS interaction.
 */

import { createConnection } from "node:net";
import type { App } from "@slack/bolt";
import type { BridgeConfig, ChannelConfig } from "./config.js";
import {
  resolveChannel,
  resolveUser,
  socketPathForUser,
  projectDirForUser,
} from "./config.js";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/** Message sent over the Unix socket to the per-user daemon. */
export interface DaemonMessage {
  /** Slack channel ID */
  channel: string;
  /** Slack thread timestamp (for threaded replies) */
  thread_ts: string;
  /** Slack user ID of the sender */
  slack_user: string;
  /** Message text */
  text: string;
  /** Resolved project directory for this user + channel */
  project_dir: string;
  /** Project name from config */
  project: string;
}

// ---------------------------------------------------------------------------
// Socket forwarding
// ---------------------------------------------------------------------------

/**
 * Send a message to a per-user daemon via Unix socket.
 * Returns true if the message was accepted, false on connection error.
 */
function forwardToDaemon(
  socketPath: string,
  msg: DaemonMessage,
): Promise<boolean> {
  return new Promise((resolve) => {
    const payload = JSON.stringify(msg) + "\n";
    const client = createConnection(socketPath, () => {
      client.write(payload, () => {
        client.end();
        resolve(true);
      });
    });

    client.on("error", (err) => {
      console.error(
        `Failed to forward to daemon at ${socketPath}: ${err.message}`,
      );
      resolve(false);
    });

    // Don't hang forever on a stale socket
    client.setTimeout(5000, () => {
      console.error(`Timeout connecting to daemon at ${socketPath}`);
      client.destroy();
      resolve(false);
    });
  });
}

// ---------------------------------------------------------------------------
// Bolt event wiring
// ---------------------------------------------------------------------------

/**
 * Register the message event handler on the Bolt app.
 * This is the core routing logic.
 */
export function registerMessageHandler(
  app: App,
  config: BridgeConfig,
): void {
  // Listen to ALL messages (no pattern filter — we route by channel + user)
  app.message(async ({ message, say }) => {
    // Only handle regular user messages (not bot messages, edits, etc.)
    if (message.subtype !== undefined) return;
    if (!("user" in message) || !("text" in message)) return;

    const channelId = message.channel;
    const slackUserId = message.user;
    const text = message.text ?? "";
    const threadTs = ("thread_ts" in message ? message.thread_ts : undefined) ?? message.ts;

    // --- Channel check ---
    const channel = resolveChannel(config, channelId);
    if (!channel) {
      // Not a configured channel — ignore silently
      return;
    }

    // --- User allowlist check ---
    const linuxUser = resolveUser(config, slackUserId);
    if (!linuxUser) {
      await say({
        text: `Sorry, you're not authorized to use CAS in this channel. Contact an admin to get added to the allowlist.`,
        thread_ts: threadTs,
      });
      return;
    }

    // --- Build daemon message ---
    const daemonMsg: DaemonMessage = {
      channel: channelId,
      thread_ts: threadTs,
      slack_user: slackUserId,
      text,
      project_dir: projectDirForUser(config, linuxUser, channel),
      project: channel.project,
    };

    // --- Forward to per-user daemon ---
    const socketPath = socketPathForUser(config, linuxUser);
    const ok = await forwardToDaemon(socketPath, daemonMsg);

    if (!ok) {
      await say({
        text: `CAS bridge for user \`${linuxUser}\` is not running. Start it with: \`sudo systemctl start cas-bridge@${linuxUser}\``,
        thread_ts: threadTs,
      });
    }
  });
}
