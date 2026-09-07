// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { applyScheme, setScheme } from "./scheme";

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.scheme;
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it("honors stored light even when the operating system is dark", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {} }));
  localStorage.setItem("commander.scheme", "light");
  expect(applyScheme()).toBe("light");
  expect(document.documentElement.dataset.scheme).toBe("light");
});

it("persists light, dark and the return to system across boot", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {} }));
  applyScheme();
  for (const [choice, expected] of [["dark", "dark"], ["light", "light"], ["system", "light"]] as const) {
    expect(setScheme(choice)).toBe(expected);
    expect(localStorage.getItem("commander.scheme")).toBe(choice);
    expect(applyScheme()).toBe(expected);
    expect(document.documentElement.dataset.scheme).toBe(expected);
  }
});

it("follows OS changes only in system mode, including after returning from an override", () => {
  const media = new EventTarget() as EventTarget & { matches: boolean };
  media.matches = false;
  vi.stubGlobal("matchMedia", () => media);
  applyScheme();
  media.matches = true;
  media.dispatchEvent(new Event("change"));
  expect(document.documentElement.dataset.scheme).toBe("dark");
  setScheme("light");
  media.dispatchEvent(new Event("change"));
  expect(document.documentElement.dataset.scheme).toBe("light");
  setScheme("system");
  expect(document.documentElement.dataset.scheme).toBe("dark");
  media.matches = false;
  media.dispatchEvent(new Event("change"));
  expect(document.documentElement.dataset.scheme).toBe("light");
});

it("defaults to light without matchMedia and treats invalid storage as system", () => {
  vi.stubGlobal("matchMedia", undefined);
  localStorage.setItem("commander.scheme", "unknown");
  expect(applyScheme()).toBe("light");
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {} }));
  expect(applyScheme()).toBe("dark");
});

it("can change the page even when browser storage is denied", () => {
  vi.stubGlobal("matchMedia", undefined);
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("denied"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("denied"); });
  expect(applyScheme()).toBe("light");
  expect(setScheme("dark")).toBe("dark");
  expect(document.documentElement.dataset.scheme).toBe("dark");
});

it("uses the system dark preference when no preference is stored", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {} }));
  expect(applyScheme()).toBe("dark");
  expect(document.documentElement.dataset.scheme).toBe("dark");
});
