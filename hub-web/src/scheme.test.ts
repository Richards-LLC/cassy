// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { applyScheme } from "./scheme";

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.scheme;
  vi.unstubAllGlobals();
});

it("uses the system dark preference when no preference is stored", () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {} }));
  expect(applyScheme()).toBe("dark");
  expect(document.documentElement.dataset.scheme).toBe("dark");
});
