import type { OperatorReply } from "./types";

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
  })[character] ?? character);
}

/** Render the durable supervisor-to-Commander replies above the composer. */
export function operatorThreadMarkup(replies: readonly OperatorReply[]): string {
  if (replies.length === 0) return "";
  return `<div class="operator-thread" aria-label="Commander conversation" aria-live="polite"><div class="operator-thread-label">Commander thread</div>${replies.map((reply) => `<article class="operator-reply" data-reply-to="${reply.reply_to}" data-notification-id="${reply.notification_id}"><header><strong>${escapeHtml(reply.operator_label || "Supervisor")}</strong><span>reply to #${reply.reply_to}</span></header><p>${escapeHtml(reply.message)}</p></article>`).join("")}</div>`;
}
