// cas-ad70: Node 22.4+ defines its own global localStorage and sessionStorage
// (experimental Web Storage). Without --localstorage-file their getters return
// undefined, and they shadow jsdom's Storage in Vitest's jsdom environment.
// Point both globals back at the jsdom window's Storage, so jsdom test files
// behave the same on every Node. Node-environment files have no jsdom and are
// left alone; on older Node the globals already are jsdom's and nothing changes.
const dom = (globalThis as { jsdom?: { window: Window } }).jsdom;
if (dom) {
  for (const name of ["localStorage", "sessionStorage"] as const) {
    const storage = dom.window[name];
    if (storage && globalThis[name] !== storage) {
      Object.defineProperty(globalThis, name, { value: storage, configurable: true, enumerable: true, writable: true });
    }
  }
}
