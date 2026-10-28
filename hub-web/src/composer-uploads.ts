import "./composer-uploads.css";

const MAX_BYTES = 10 * 1024 * 1024;
interface PendingFile { id: string; file: File; preview?: string; thumbnail?: string; path?: string }
type Upload = (id: string, name: string, data: string) => Promise<{ path: string; artifact_id: string }>;

/** Bytes remain in this page through socket reconnects and shell replacements. */
export class ComposerUploads {
  private files: PendingFile[] = [];
  private root?: ParentNode;
  private error = "";
  private sending = false;

  bind(root: ParentNode): void { this.root = root; this.paint(); }
  hasFiles(): boolean { return this.files.length > 0; }
  add(files: File[]): void {
    if (this.sending) return;
    this.error = "";
    for (const file of files) {
      if (!file.size || file.size > MAX_BYTES) {
        this.error = `${file.name}: choose a non-empty file up to 10 MiB.`;
        continue;
      }
      if (this.files.length >= 8) { this.error = "Send up to 8 files at a time."; break; }
      const entry: PendingFile = { id: crypto.randomUUID(), file };
      // Never mount SVG/HTML from a paste. Raster thumbnails are local object URLs.
      if (/^image\/(png|jpeg|gif|webp)$/.test(file.type) && typeof URL.createObjectURL === "function") entry.thumbnail = URL.createObjectURL(file);
      this.files.push(entry);
    }
    this.paint();
  }
  addText(text: string): void {
    const file = new File([text], "pasted-text.txt", { type: "text/plain" });
    this.add([file]);
    const entry = this.files.find(item => item.file === file);
    if (entry) entry.preview = text.slice(0, 240);
    this.paint();
  }
  clear(): void {
    for (const entry of this.files) if (entry.thumbnail) URL.revokeObjectURL(entry.thumbnail);
    this.files = []; this.error = ""; this.paint();
  }
  async prepare(text: string, upload: Upload): Promise<string> {
    if (this.sending) throw new Error("Files are already being uploaded.");
    this.sending = true; this.error = ""; this.paint();
    try {
      for (const entry of this.files) {
        if (!entry.path) {
          const data = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(String(reader.result).split(",")[1]!);
            reader.onerror = () => reject(new Error(`Couldn't read ${entry.file.name}. Choose the file again.`));
            reader.readAsDataURL(entry.file);
          });
          try {
            entry.path = (await upload(entry.id, entry.file.name, data)).path;
          } catch (error) {
            throw new Error(`${entry.file.name}: ${error instanceof Error ? error.message : "The upload failed."}`);
          }
        }
      }
      return [text, ...this.files.map(entry => `Attached file: ${entry.path}`)].filter(Boolean).join("\n\n");
    } catch (error) {
      this.error = `Files were not sent. ${error instanceof Error ? error.message : "The upload failed."} Your files are kept here.`;
      throw new Error(this.error);
    } finally { this.sending = false; this.paint(); }
  }
  private paint(): void {
    const field = this.root?.querySelector<HTMLTextAreaElement>("#message-text");
    if (!field) return;
    let strip = this.root?.querySelector<HTMLElement>(".composer-uploads");
    if (!strip) {
      strip = document.createElement("div"); strip.className = "composer-uploads";
      field.before(strip);
    }
    strip.replaceChildren();
    strip.hidden = !this.files.length && !this.error;
    const list = document.createElement("div"); list.className = "composer-upload-files";
    strip.append(list);
    for (const entry of this.files) {
      const chip = document.createElement("div"); chip.className = "composer-file";
      if (entry.thumbnail) { const image = document.createElement("img"); image.src = entry.thumbnail; image.alt = ""; chip.append(image); }
      const label = document.createElement("span"); label.textContent = `${entry.file.name} · ${Math.ceil(entry.file.size / 1024)} KiB`;
      chip.append(label);

      const remove = document.createElement("button"); remove.type = "button"; remove.textContent = "Remove";
      remove.setAttribute("aria-label", `Remove ${entry.file.name}`); remove.disabled = this.sending;
      remove.onclick = () => { if (entry.thumbnail) URL.revokeObjectURL(entry.thumbnail); this.files = this.files.filter(item => item !== entry); this.paint(); };
      chip.append(remove);
      if (entry.preview) { const preview = document.createElement("pre"); preview.textContent = entry.preview; chip.append(preview); }
      list.append(chip);
    }
    const status = document.createElement("p"); status.setAttribute("role", "status");
    status.textContent = this.error || (this.sending ? "Uploading files…" : this.files.length ? "Files stay here through reconnects. Send before closing this page." : "");
    if (this.error) status.className = "upload-error";
    strip.append(status);
  }
}
const drafts = new Map<string, ComposerUploads>();
export function draftFor(thread: string): ComposerUploads {
  let draft = drafts.get(thread);
  if (!draft) { draft = new ComposerUploads(); drafts.set(thread, draft); }
  return draft;
}
