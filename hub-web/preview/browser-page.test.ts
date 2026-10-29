import { afterEach, expect, it, vi } from "vitest";
import { JSDOM } from "jsdom";
import { HubDouble } from "../e2e/journeys/hub-double";

afterEach(() => vi.unstubAllGlobals());

it("serves the current HubDouble HTTP and socket fixtures in a browser", async () => {
  const dom = new JSDOM("", { url: "https://preview.test/commander/" });
  vi.stubGlobal("window", dom.window);
  vi.stubGlobal("location", dom.window.location);
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
    expect(await (await window.fetch("https://atlas.test/v1/sessions")).json()).toEqual({ sessions: [] });
    expect(await (await window.fetch("https://api.github.com/repos/Richards-LLC/cassy/releases/latest")).json()).toEqual({ tag_name: "v3.87.0" });
    const socket = new window.WebSocket("wss://atlas.test/v1/attach");
    await new Promise<void>(resolve => socket.addEventListener("open", () => resolve(), { once: true }));
    expect(hub.machineSocketOpens.get("atlas")).toBe(1);
    socket.close();
    expect(network).not.toHaveBeenCalled();
  } finally {
    dom.window.close();
  }
});
