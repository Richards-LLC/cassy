// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { composerMarkup } from "./composer-markup";
import { ComposerUploads } from "./composer-uploads";
import { ConversationHistory } from "./conversation-history";
import { ConversationView } from "./conversation-view";
import { installAttachmentSheet } from "./attachment-sheet";
import type { ArtifactRef } from "./types";

it("previews pasted content, keeps failed uploads across rebinding, and renders sent files without exposing host paths", async () => {
  const draft = new ComposerUploads();
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  draft.add([new File(["notes"], "notes.txt", { type: "text/plain" })]);
  expect(document.body.textContent).toContain("notes.txt");
  const upload = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ path: "/host/project/art-one/notes.txt", artifact_id: "art-one", mime: "text/plain" });
  await expect(draft.prepare("See this", upload)).rejects.toThrow("offline");
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  expect(document.body.textContent).toContain("notes.txt");
  const text = await draft.prepare("See this", upload);
  expect(text).toContain("/host/project/art-one/notes.txt");
  draft.add([new File(["image"], "screenshot.bin", { type: "application/octet-stream" })]);
  upload.mockResolvedValueOnce({ path: "/host/project/art-image/screenshot.bin", artifact_id: "art-image", mime: "image/png" });
  const prepared = await draft.prepare("See this", upload);
  const files: ArtifactRef[] = [];
  const unregister = installAttachmentSheet((sheet, file) => { files.push(file); return sheet; });
  const history = new ConversationHistory();
  const view = new ConversationView(document, history, { supervisor: "supervisor" });
  try {
    history.submit("send", "supervisor", prepared);
    view.update();
    expect(view.element.textContent).toContain("See this");
    expect(view.element.textContent).toContain("notes.txt");
    expect(view.element.textContent).toContain("screenshot.bin");
    expect(view.element.textContent).not.toContain("/host/");
    expect(history.preview()).not.toContain("/host/");
    expect(files.map(file => [file.artifact_id, file.mime, file.size_bytes])).toEqual([
      ["art-one", "text/plain", 5], ["art-image", "image/png", 5],
    ]);
    expect(view.element.querySelector('a[data-artifact-id="art-image"]')?.getAttribute("href")).toBe("#artifact:art-image");
    history.submit("legacy", "supervisor", "Attached files\n\nAttached file: /home/operator/.cas/artifacts/commander/session/art-legacy/photo.png");
    view.update();
    expect(view.element.textContent).not.toContain("/home/operator");
    expect(view.element.querySelector('a[data-artifact-id="art-legacy"]')).not.toBeNull();
  } finally { view.dispose(); unregister(); }
  expect(upload.mock.calls[0]![0]).toBe(upload.mock.calls[1]![0]);
  draft.clear();
  expect(document.body.textContent).not.toContain("notes.txt");
});

it("turns only long text into a removable preview and refuses oversized files visibly", () => {
  const draft = new ComposerUploads();
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  draft.addText("Long content ".repeat(1500));
  expect(document.body.textContent).toContain("Long content");
  document.querySelector<HTMLButtonElement>('[aria-label="Remove pasted-text.txt"]')!.click();
  expect(draft.hasFiles()).toBe(false);
  draft.add([new File([new Uint8Array(10 * 1024 * 1024 + 1)], "too-big.png")]);
  expect(document.body.textContent).toContain("10 MiB");
  expect(draft.hasFiles()).toBe(false);
});

it("names the refused file among several attachments without prescribing a reconnect", async () => {
  const draft = new ComposerUploads();
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  draft.add([new File(["ok"], "notes.txt"), new File(["bad"], "archive.bin")]);
  const upload = vi.fn().mockResolvedValueOnce({ path: "/host/notes.txt" })
    .mockRejectedValueOnce(new Error("This file type is not supported."));
  await expect(draft.prepare("", upload)).rejects.toThrow("archive.bin: This file type is not supported.");
  expect(document.querySelector(".upload-error")?.textContent).toContain("archive.bin");
  expect(document.querySelector(".upload-error")?.textContent).not.toContain("reconnect");
  expect(draft.hasFiles()).toBe(true);
});
