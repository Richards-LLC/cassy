// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { composerMarkup } from "./composer-markup";
import { ComposerUploads } from "./composer-uploads";

it("previews pasted content, keeps failed uploads across rebinding, and sends stable local paths", async () => {
  const draft = new ComposerUploads();
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  draft.add([new File(["notes"], "notes.txt", { type: "text/plain" })]);
  expect(document.body.textContent).toContain("notes.txt");
  const upload = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ path: "/host/project/notes.txt", artifact_id: "art-one" });
  await expect(draft.prepare("See this", upload)).rejects.toThrow("offline");
  document.body.innerHTML = composerMarkup("supervisor");
  draft.bind(document.body);
  expect(document.body.textContent).toContain("notes.txt");
  expect(await draft.prepare("See this", upload)).toContain("/host/project/notes.txt");
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
