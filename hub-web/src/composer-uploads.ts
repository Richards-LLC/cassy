export class ComposerUploads {
  bind(_root: ParentNode): void {}
  add(_files: File[]): void {}
  addText(_text: string): void {}
  hasFiles(): boolean { return false; }
  async prepare(text: string, _upload: (id: string, name: string, data: string) => Promise<{path: string; artifact_id: string}>): Promise<string> { return text; }
  clear(): void {}
}
