// @vitest-environment jsdom
// cas-eaa3: Commander ↔ Explorer. The switcher reaches Explorer on the cloud
// origin for the current project and never carries a credential.
import { describe, expect, it, vi } from "vitest";
import { DEFAULT_EXPLORER_ORIGIN, EXPLORER_TARGET, appSwitcherMarkup, closeAppSwitcherAfterFollow, cloudProjectId, explorerOrigin, explorerUrl } from "./app-switcher";
import { cloudBrandSwitcher } from "./cloud-brand";

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
    expect([nav.id, nav.getAttribute("aria-label"), nav.hasAttribute("popover")]).toEqual(["app-switcher", "Cassy Cloud apps", true]);
    const [commander, explorer] = [...nav.children] as HTMLElement[];
    expect([commander.querySelector(".app-switcher-name")!.textContent, commander.getAttribute("aria-current"), commander.tagName]).toEqual(["Commander", "page", "SPAN"]);
    expect(explorer.tagName).toBe("A");
    expect(explorer.querySelector(".app-switcher-name")!.textContent).toBe("Explorer");
    expect(explorer.querySelector(".sr-only")!.textContent).toBe(" (opens in a new tab)");
    expect(explorer.getAttribute("target")).toBe(EXPLORER_TARGET);
    expect(explorer.getAttribute("rel")).toBe("noopener noreferrer");
    expect(explorer.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(explorer.getAttribute("href")).toBe("https://petra-stella-cloud.vercel.app/explorer/tasks?project_id=github.com%2Facme%2Fwidget");
  });

  it("opens from the list header's lockup, which costs the header no row or width", () => {
    const button = node(cloudBrandSwitcher());
    expect([button.tagName, button.id, button.getAttribute("popovertarget"), button.getAttribute("aria-label")]).toEqual(["BUTTON", "app-switcher-toggle", "app-switcher", "Cassy Cloud apps"]);
    // Label in name: the visible wordmark is part of the accessible name.
    expect(button.textContent).toBe("Cassy Cloud");
    expect(button.querySelector("img.cloud-brand-mark")!.getAttribute("alt")).toBe("");
  });

  it("names Explorer as unavailable when no cloud origin is configured", () => {
    const explorer = node(appSwitcherMarkup(null)).lastElementChild as HTMLElement;
    expect([explorer.tagName, explorer.getAttribute("aria-disabled")]).toEqual(["SPAN", "true"]);
  });
});

describe("closeAppSwitcherAfterFollow", () => {
  // cas-2a26 (journey eval 3.51.0 F02): Explorer opens in its own tab, so the
  // menu closes in Commander's tab and focus returns to the apps button.
  it("hides the switcher and refocuses the apps button when Explorer is followed", () => {
    document.body.innerHTML = cloudBrandSwitcher() + appSwitcherMarkup(explorerUrl(DEFAULT_EXPLORER_ORIGIN));
    const nav = document.getElementById("app-switcher")!;
    const hide = vi.fn();
    Object.assign(nav, { hidePopover: hide });
    const link = nav.querySelector<HTMLAnchorElement>("a.app-switcher-item")!;
    expect(closeAppSwitcherAfterFollow(link.querySelector(".app-switcher-name"))).toBe(true);
    expect(hide).toHaveBeenCalledTimes(1);
    expect(document.activeElement?.id).toBe("app-switcher-toggle");
  });

  it("leaves the menu alone for Commander's own row and any other click", () => {
    document.body.innerHTML = cloudBrandSwitcher() + appSwitcherMarkup(null) + '<a id="other" href="/x">x</a>';
    const nav = document.getElementById("app-switcher")!;
    const hide = vi.fn();
    Object.assign(nav, { hidePopover: hide });
    for (const target of [nav.querySelector('[aria-current="page"]'), nav.querySelector('[aria-disabled="true"]'), document.getElementById("other"), null]) {
      expect(closeAppSwitcherAfterFollow(target)).toBe(false);
    }
    expect(hide).not.toHaveBeenCalled();
  });
});
