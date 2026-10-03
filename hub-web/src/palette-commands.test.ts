// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { controlCommandCopy, paletteEnterTarget, sessionJumpCommandMarkup } from "./palette-commands";

function row(markup: string): HTMLButtonElement {
  const host = document.createElement("div");
  host.innerHTML = markup;
  return host.firstElementChild as HTMLButtonElement;
}

describe("palette Jump to rows (cas-cfcb)", () => {
  const studio = { id: "studio-mac", label: "Studio Mac · macOS" };
  const session = { name: "calm-otter-4", supervisor: "calm-otter-4", project_dir: "/projects/gabber-studio" };

  it("leads with the project, keeps the codename secondary, and indexes both for the filter (3.30.0 F2)", () => {
    const command = row(sessionJumpCommandMarkup(studio, session));
    expect(command.querySelector("span")?.textContent).toBe("Jump to gabber-studio");
    expect(command.querySelector("small")?.textContent).toBe("calm-otter-4 · Studio Mac · macOS");
    expect(command.dataset.searchText).toBe("gabber-studio calm-otter-4");
    expect(command.dataset.paletteMachine).toBe("studio-mac");
    expect(command.dataset.paletteSession).toBe("calm-otter-4");
  });

  it("keeps the summary after the codename and machine, with its description searchable", () => {
    const command = row(sessionJumpCommandMarkup(studio, session, { title: "Release train", description: "Cut 3.28", phase: "building" }));
    expect(command.querySelector("small")?.textContent).toBe("calm-otter-4 · Studio Mac · macOS · Release train · building");
    expect(command.querySelector("small")?.getAttribute("title")).toBe("Cut 3.28");
    expect(command.dataset.searchText).toBe("gabber-studio calm-otter-4 Release train Cut 3.28 building");
  });

  it("leads with the session name when no project is named, without a placeholder, and escapes every value", () => {
    const command = row(sessionJumpCommandMarkup({ id: 'a"b', label: "<Atlas>" }, { name: "s<1>", supervisor: 'x"y' }));
    expect(command.querySelector("small")?.textContent).toBe("<Atlas>");
    expect(command.dataset.searchText).toBe('x"y');
    expect(command.dataset.paletteMachine).toBe('a"b');
    expect(command.querySelector("span")?.textContent).toBe("Jump to s<1>");
    expect(command.querySelectorAll("*")).toHaveLength(2);
  });

  it("names the session by its codename when the hub reports no supervisor", () => {
    const command = row(sessionJumpCommandMarkup(studio, { name: "calm-otter-4", supervisor: "", project_dir: "/projects/gabber-studio" }));
    expect(command.querySelector("span")?.textContent).toBe("Jump to gabber-studio");
    expect(command.querySelector("small")?.textContent).toBe("calm-otter-4 · Studio Mac · macOS");
  });
});

describe("palette control command (journey F16)", () => {
  it("names what this device can do, keeping the control term in the hint", () => {
    expect(controlCommandCopy({ heldByMe: true, forceTakeover: false })).toEqual({ title: "Let other devices type here", hint: "Release control of this conversation" });
    expect(controlCommandCopy({ heldByMe: false, forceTakeover: false })).toEqual({ title: "Type here from this device", hint: "Take control of this conversation" });
    expect(controlCommandCopy({ heldByMe: false, forceTakeover: true, controller: "Studio iPad" })).toEqual({ title: "Type here from this device", hint: "Force takeover from Studio iPad" });
    expect(controlCommandCopy({ heldByMe: false, forceTakeover: true })).toEqual({ title: "Type here from this device", hint: "Force takeover" });
  });

  it("says why when the command is unavailable", () => {
    expect(controlCommandCopy({ heldByMe: false, forceTakeover: false, disabledReason: "Studio iPad is in control." })).toEqual({ title: "Type here from this device", hint: "Studio iPad is in control." });
  });

  it("never says session", () => {
    for (const heldByMe of [true, false]) for (const forceTakeover of [true, false]) {
      const copy = controlCommandCopy({ heldByMe, forceTakeover, controller: "Studio iPad" });
      expect(`${copy.title} ${copy.hint}`).not.toMatch(/session/i);
    }
  });
});

describe("palette Enter target with no filter (cas-786a, journey F30)", () => {
  const atlas = { id: "atlas", label: "Atlas · Linux" };
  const studio = { id: "studio", label: "Studio Mac · macOS" };
  const forge = { id: "forge", label: "Forge · Linux" };
  const casSrc = row(sessionJumpCommandMarkup(atlas, { name: "s1", supervisor: "patient-pelican-9", project_dir: "/projects/cas-src" }, undefined, { current: true }));
  const lighthouse = row(sessionJumpCommandMarkup(forge, { name: "s2", supervisor: "quiet-heron-7", project_dir: "/projects/lighthouse" }));
  const gabber = row(sessionJumpCommandMarkup(studio, { name: "s3", supervisor: "calm-otter-4", project_dir: "/projects/gabber-studio" }, undefined, { needsYou: true }));
  const settings = row('<button type="button" class="palette-command" data-palette-action="paired"><span>Paired machines</span></button>');

  it("marks the open conversation as the palette's current item, its description unchanged", () => {
    expect(casSrc.dataset.paletteCurrent).toBe("true");
    expect(casSrc.getAttribute("aria-current")).toBe("true");
    expect(casSrc.querySelector("small")?.textContent).toBe("patient-pelican-9 · Atlas · Linux");
    expect(lighthouse.hasAttribute("aria-current")).toBe(false);
    expect(lighthouse.dataset.paletteCurrent).toBeUndefined();
    expect(gabber.dataset.paletteNeedsYou).toBe("true");
  });

  it("goes to the next conversation that needs the operator, never the open one", () => {
    expect(paletteEnterTarget([casSrc, lighthouse, gabber, settings], "")).toBe(gabber);
  });

  it("goes to the first other conversation when none needs the operator", () => {
    const quiet = row(sessionJumpCommandMarkup(studio, { name: "s3", supervisor: "calm-otter-4", project_dir: "/projects/gabber-studio" }));
    expect(paletteEnterTarget([casSrc, lighthouse, quiet, settings], "  ")).toBe(lighthouse);
    // With no other conversation, no setting becomes a surprise default: Enter stays on the open one.
    expect(paletteEnterTarget([casSrc, settings], "")).toBe(casSrc);
  });

  it("with a filter, keeps the first row on screen unless it only jumps to the open conversation", () => {
    expect(paletteEnterTarget([lighthouse, settings], "light")).toBe(lighthouse);
    expect(paletteEnterTarget([casSrc, settings], "a")).toBe(settings);
    expect(paletteEnterTarget([casSrc], "pelican")).toBe(casSrc);
    expect(paletteEnterTarget([], "zzz")).toBeUndefined();
  });
});
