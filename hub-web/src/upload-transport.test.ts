import { afterEach, expect, it, vi } from "vitest";
import { uploadRequest } from "./upload-transport";

// The browser transport is the only double: response/error parsing stays real.
class UploadSocket {
  static latest: UploadSocket;
  upload = { onprogress: (_event: { lengthComputable: boolean; loaded: number; total: number }) => {}, onload: () => {} };
  onload = () => {}; onerror = () => {}; onabort = () => {};
  status = 415; statusText = "Unsupported Media Type";
  responseText = '{"error":"unsupported_file","detail":"Choose a supported file."}';
  headers = new Headers();
  aborted = false;
  constructor() { UploadSocket.latest = this; }
  open = vi.fn(); send = vi.fn();
  setRequestHeader(name: string, value: string) { this.headers.set(name, value); }
  getAllResponseHeaders() { return "content-type: application/json\r\nx-cas-request-id: request-id\r\n"; }
  abort() { this.aborted = true; this.onabort(); }
}
afterEach(() => vi.unstubAllGlobals());
it("reports actual transfer fractions and preserves signed headers and refusal responses", async () => {
  vi.stubGlobal("XMLHttpRequest", UploadSocket);
  const progress = vi.fn();
  const result = uploadRequest(new URL("https://hub.test/uploads/id"), { method: "POST", headers: { DPoP: "signed-proof" }, body: "file-data" }, progress);
  const socket = UploadSocket.latest;
  expect(socket.headers.get("dpop")).toBe("signed-proof");
  expect(socket.send).toHaveBeenCalledWith("file-data");
  socket.upload.onprogress({ lengthComputable: true, loaded: 5, total: 10 });
  socket.upload.onload();
  expect(progress.mock.calls.flat()).toEqual([0, 0.5, 1]);
  socket.onload();
  const response = await result;
  expect(response.status).toBe(415);
  expect(response.headers.get("x-cas-request-id")).toBe("request-id");
  expect(await response.json()).toMatchObject({ error: "unsupported_file" });
});
it("aborts the upload transport when the request deadline cancels it", async () => {
  vi.stubGlobal("XMLHttpRequest", UploadSocket);
  const parent = new AbortController();
  const result = uploadRequest(new URL("https://hub.test/uploads/id"), { signal: parent.signal, body: "file" }, () => {});
  const rejected = expect(result).rejects.toMatchObject({ name: "TimeoutError" });
  parent.abort(new DOMException("Deadline exceeded", "TimeoutError"));
  await rejected;
  expect(UploadSocket.latest.aborted).toBe(true);
});
