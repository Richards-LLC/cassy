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
  const parts = message.split("\n\n");
  const attachments: ArtifactRef[] = [];
  while (parts.length) {
    const part = parts.at(-1)!;
    const named = /^Attached file: \[([^\]\r\n]+)\]\(<([^>\r\n]+)> "([a-z]+\/[a-z0-9.+-]+); size=(\d+)"\)$/.exec(part);
    const path = named?.[2] ?? /^Attached file: (\/[^\r\n]+)$/.exec(part)?.[1];
    if (!path) break;
    const stored = /\/(art-[a-zA-Z0-9-]+)\/([^/]+)$/.exec(path);
    if (!stored) break;
    const name = stored[2]!;
    attachments.unshift({
      artifact_id: stored[1]!, name,
      mime: named?.[3] ?? legacyMime[name.split(".").at(-1)!.toLowerCase()] ?? "application/octet-stream",
      size_bytes: named ? Number(named[4]) : -1,
      sha256: "",
    });
    parts.pop();
  }
  const text = parts.join("\n\n");
  return { text: attachments.length && text === "Attached files" ? "" : text, attachments };
}

/** Compact copy for previews and quotes; transport text stays untouched. */
export function sentMessageLabel(message: string): string {
  const { text, attachments } = sentAttachments(message);
  return [text, ...attachments.map(file => file.name)].filter(Boolean).join(" · ");
}
