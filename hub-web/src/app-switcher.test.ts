// @vitest-environment jsdom
// cas-eaa3: Commander ↔ Explorer. The switcher reaches Explorer on the cloud
// origin for the current project and never carries a credential.
import { describe, expect, it } from "vitest";
import { DEFAULT_EXPLORER_ORIGIN, EXPLORER_TARGET, appSwitcherMarkup, cloudProjectId, explorerOrigin, explorerUrl } from "./app-switcher";

const node = (html: string) => { const host = document.createElement("div"); host.innerHTML = html; return host.firstElementChild as HTMLElement; };

describe("explorerOrigin", () => {
  it("accepts a bare https origin only", () => {
    expect(explorerOrigin(DEFAULT_EXPLORER_ORIGIN)).toBe("https://petra-stella-cloud.vercel.app");
    expect(explorerOrigin("https://cloud.example/")).toBe("https://cloud.example");
    for (const bad of ["http://cloud.example", "https://user:pw@cloud.example", "https://cloud.example/explorer", "https://cloud.example/?t=1", "https://cloud.example/#x", "javascript:alert(1)", "", null, undefined]) {
      expect(explorerOrigin(bad), String(bad)).toBeNull();
    }
  });
});

describe("explorerUrl", () => {
  it("opens the current project's tasks, else Explorer's home", () => {
    expect(explorerUrl(DEFAULT_EXPLORER_ORIGIN, "github.com/richards-llc/cassy")).toBe("https://petra-stella-cloud.vercel.app/explorer/tasks?project_id=github.com%2Frichards-llc%2Fcassy");
    expect(explorerUrl(DEFAULT_EXPLORER_ORIGIN, "gabber-studio")).toBe("https://petra-stella-cloud.vercel.app/explorer/tasks?project_id=gabber-studio");
    expect(explorerUrl(DEFAULT_EXPLORER_ORIGIN)).toBe("https://petra-stella-cloud.vercel.app/explorer");
  });

  it("carries nothing but the path and a well-formed project id", () => {
    for (const hostile of ["x?token=abc", "x#device=1", "a b", "../../api/operator", "-flag", "x&teamId=1", "x".repeat(201)]) {
      expect(cloudProjectId(hostile), hostile).toBeNull();
      expect(explorerUrl(DEFAULT_EXPLORER_ORIGIN, hostile)).toBe("https://petra-stella-cloud.vercel.app/explorer");
    }
    const url = new URL(explorerUrl(DEFAULT_EXPLORER_ORIGIN, "github.com/acme/widget"));
    expect([url.username, url.password, url.hash]).toEqual(["", "", ""]);
    expect([...url.searchParams.keys()]).toEqual(["project_id"]);
  });
});

describe("appSwitcherMarkup", () => {
  it("marks Commander current and opens Explorer in its own tab without a referrer", () => {
    const nav = node(appSwitcherMarkup(explorerUrl(DEFAULT_EXPLORER_ORIGIN, "github.com/acme/widget")));
    expect(nav.getAttribute("aria-label")).toBe("Cassy Cloud apps");
    const [commander, explorer] = [...nav.children] as HTMLElement[];
    expect([commander.textContent, commander.getAttribute("aria-current"), commander.tagName]).toEqual(["Commander", "page", "SPAN"]);
    expect(explorer.tagName).toBe("A");
    expect(explorer.textContent).toBe("Explorer↗ (opens in a new tab)");
    expect(explorer.getAttribute("target")).toBe(EXPLORER_TARGET);
    expect(explorer.getAttribute("rel")).toBe("noopener noreferrer");
    expect(explorer.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(explorer.getAttribute("href")).toBe("https://petra-stella-cloud.vercel.app/explorer/tasks?project_id=github.com%2Facme%2Fwidget");
  });

  it("names Explorer as unavailable when no cloud origin is configured", () => {
    const explorer = node(appSwitcherMarkup(null)).lastElementChild as HTMLElement;
    expect([explorer.tagName, explorer.getAttribute("aria-disabled")]).toEqual(["SPAN", "true"]);
  });
});
