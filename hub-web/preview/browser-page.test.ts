import { afterEach, expect, it, vi } from "vitest";
import { JSDOM } from "jsdom";
import { HubDouble } from "../e2e/journeys/hub-double";

afterEach(() => vi.unstubAllGlobals());

it("names the missing hub frame when receipt waiting times out", async () => {
  const timeout = Object.assign(new Error("page.waitForFunction: Timeout 5000ms exceeded"), { name: "TimeoutError" });
  const page = { waitForFunction: vi.fn().mockRejectedValue(timeout) };
  const hub = new HubDouble(page as never, { machines: [] });
  await expect(hub.received(4711)).rejects.toMatchObject({
    message: expect.stringContaining("hub frame 4711"),
    cause: timeout,
  });
});

it("serves the current HubDouble HTTP and socket fixtures in a browser", async () => {
  const dom = new JSDOM("", { url: "https://preview.test/commander/" });
  vi.stubGlobal("window", dom.window);
  vi.stubGlobal("location", dom.window.location);
  // Use one DOM realm for targets and events, including Node versions without CloseEvent.
  for (const name of ["EventTarget", "Event", "MessageEvent", "CloseEvent"] as const) {
    vi.stubGlobal(name, dom.window[name]);
  }
  const network = vi.fn(() => Promise.reject(new Error("unexpected live request")));
  Object.assign(dom.window, { fetch: network });
  const { PreviewPage } = await import("./browser-page");
  const page = new PreviewPage();
  page.installTransport();
  const hub = new HubDouble(page as never, {
    machines: [{ id: "atlas", label: "Atlas", sessions: [] }],
    multiplex: true,
  });
  try {
    await hub.install();
    expect(await (await window.fetch("https://atlas.test/v1/sessions")).json()).toMatchObject({ sessions: [] });
    expect(await (await window.fetch("https://api.github.com/repos/Richards-LLC/cassy/releases/latest")).json()).toEqual({ tag_name: "v3.87.0" });
    const socket = new window.WebSocket("wss://atlas.test/v1/attach");
    await new Promise<void>(resolve => socket.addEventListener("open", () => resolve(), { once: true }));
    expect(hub.machineSocketOpens.get("atlas")).toBe(1);
    const closed = new Promise<CloseEvent>(resolve => socket.addEventListener("close", resolve, { once: true }));
    socket.close(1000, "test complete");
    await expect(closed).resolves.toMatchObject({ code: 1000, reason: "test complete", wasClean: true });
    expect(network).not.toHaveBeenCalled();
  } finally {
    dom.window.close();
  }
});
