// @vitest-environment jsdom
import { expect, it } from "vitest";

// cas-ad70: Node 22.4+ defines its own global localStorage and sessionStorage
// (experimental Web Storage). Without --localstorage-file the getters return
// undefined, and they shadow jsdom's Storage in Vitest's jsdom environment, so
// browser code under test saw no storage on a default Node 26 run. Every
// jsdom test file must see jsdom's working Storage, whatever the host Node does.
it("jsdom test files see jsdom's localStorage and sessionStorage, not the host Node's", () => {
  const dom = (globalThis as { jsdom?: { window: Window } }).jsdom;
  expect(dom, "Vitest's jsdom environment exposes its window").toBeDefined();
  for (const name of ["localStorage", "sessionStorage"] as const) {
    const storage = globalThis[name];
    expect(storage, `${name} is defined`).toBeDefined();
    expect(storage, `${name} is jsdom's Storage`).toBe(dom!.window[name]);
    storage.setItem("cas-ad70", "kept");
    expect(storage.getItem("cas-ad70")).toBe("kept");
    storage.removeItem("cas-ad70");
    expect(storage.getItem("cas-ad70")).toBeNull();
  }
});
