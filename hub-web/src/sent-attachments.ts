import type { ArtifactRef } from "./types";

/** Keep the host's resolvable path on the wire, with durable display metadata.
 * Markdown also gives older readers a named file rather than a bare path. */
export function sentAttachmentReference(file: { path: string; mime: string; size: number }): string {
  const name = file.path.split("/").at(-1)!;
  return `Attached file: [${name}](<${file.path}> "${file.mime}; size=${file.size}")`;
}

const legacyMime: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", avif: "image/avif",
  pdf: "application/pdf", txt: "text/plain", md: "text/plain", zip: "application/zip",
};

/** Decode both current references and the old bare-path upload suffix. This
 * runs on live sends and durable history alike; object URLs never enter history. */
export function sentAttachments(message: string): { text: string; attachments: ArtifactRef[] } {
  const { text, files } = sentAttachmentFiles(message);
  return { text, attachments: files.map(file => file.ref) };
}

/** The same decoding with each file's host path, for re-sending an edited
 * message only. The path stays in the composer's state, never in its text. */
export function sentAttachmentFiles(message: string): { text: string; files: { ref: ArtifactRef; path: string }[] } {
  const parts = message.split("\n\n");
  const files: { ref: ArtifactRef; path: string }[] = [];
  while (parts.length) {
    const part = parts.at(-1)!;
    const named = /^Attached file: \[([^\]\r\n]+)\]\(<([^>\r\n]+)> "([a-z]+\/[a-z0-9.+-]+); size=(\d+)"\)$/.exec(part);
    const path = named?.[2] ?? /^Attached file: (\/[^\r\n]+)$/.exec(part)?.[1];
    if (!path) break;
    const stored = /\/(art-[a-zA-Z0-9-]+)\/([^/]+)$/.exec(path);
    if (!stored) break;
    const name = stored[2]!;
    files.unshift({ path, ref: {
      artifact_id: stored[1]!, name,
      mime: named?.[3] ?? legacyMime[name.split(".").at(-1)!.toLowerCase()] ?? "application/octet-stream",
      size_bytes: named ? Number(named[4]) : -1,
      sha256: "",
    } });
    parts.pop();
  }
  const text = parts.join("\n\n");
  return { text: files.length && text === "Attached files" ? "" : text, files };
}

/** Compact copy for previews and quotes; transport text stays untouched. */
export function sentMessageLabel(message: string): string {
  const command = nativeCommandReceipt(message);
  if (command) return command.text;
  const { text, attachments } = sentAttachments(message);
  return [text, ...attachments.map(file => file.name)].filter(Boolean).join(" · ");
}

/** Claude's local command transcript is a receipt, not an operator message. */
export function nativeCommandReceipt(message: string): { label: string; text: string } | undefined {
  const text = message.trim();
  const local = /^<(local-command-stdout|local-command-caveat)>([\s\S]*)<\/\1>$/.exec(text);
  if (local) return local[1] === "local-command-caveat"
    ? { label: "Terminal command", text: "Run directly in Claude Code" }
    : { label: "Terminal result", text: local[2]!.trim() };
  const fields = new Map<string, string>();
  let end = 0;
  for (const part of text.matchAll(/<(command-name|command-message|command-args)>([^<>]*)<\/\1>/g)) {
    if (text.slice(end, part.index).trim() || fields.has(part[1]!)) return undefined;
    fields.set(part[1]!, part[2]!.trim()); end = part.index + part[0].length;
  }
  const name = fields.get("command-name");
  if (!name || !/^\/[a-z][\w:-]*$/i.test(name) || text.slice(end).trim()) return undefined;
  return { label: "Terminal command", text: [name, fields.get("command-args")].filter(Boolean).join(" ") };
}
