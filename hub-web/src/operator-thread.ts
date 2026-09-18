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
  return `<div class="operator-thread" aria-label="Commander conversation" aria-live="polite"><div class="operator-thread-label">Commander thread</div>${replies.map((reply) => `<article class="operator-reply" data-kind="${reply.kind ?? "answer"}" data-reply-to="${reply.reply_to ?? ""}" data-notification-id="${reply.notification_id}"><header><strong>${escapeHtml(reply.operator_label || "Supervisor")}</strong><span>${escapeHtml(reply.kind ?? "answer")}${reply.reply_to === null ? "" : ` · reply to #${reply.reply_to}`}</span></header><p>${escapeHtml(reply.message)}</p>${(reply.attachments ?? []).map((attachment) => `<div class="operator-attachment"><a href="#artifact:${encodeURIComponent(attachment.artifact_id)}" data-artifact-id="${escapeHtml(attachment.artifact_id)}">${escapeHtml(attachment.name)}</a></div>`).join("")}</article>`).join("")}</div>`;
}
