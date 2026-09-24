// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { sessionJumpCommandMarkup } from "./palette-commands";

function row(markup: string): HTMLButtonElement {
  const host = document.createElement("div");
  host.innerHTML = markup;
  return host.firstElementChild as HTMLButtonElement;
}

describe("palette Jump to rows (cas-cfcb)", () => {
  const studio = { id: "studio-mac", label: "Studio Mac · macOS" };
  const session = { name: "calm-otter-4", supervisor: "calm-otter-4", project_dir: "/projects/gabber-studio" };

  it("names the project first on the second line and indexes it for the filter", () => {
    const command = row(sessionJumpCommandMarkup(studio, session));
    expect(command.querySelector("span")?.textContent).toBe("Jump to calm-otter-4");
    expect(command.querySelector("small")?.textContent).toBe("gabber-studio · Studio Mac · macOS");
    expect(command.dataset.searchText).toBe("gabber-studio calm-otter-4");
    expect(command.dataset.paletteMachine).toBe("studio-mac");
    expect(command.dataset.paletteSession).toBe("calm-otter-4");
  });

  it("keeps the summary after the project and machine, with its description searchable", () => {
    const command = row(sessionJumpCommandMarkup(studio, session, { title: "Release train", description: "Cut 3.28", phase: "building" }));
    expect(command.querySelector("small")?.textContent).toBe("gabber-studio · Studio Mac · macOS · Release train · building");
    expect(command.querySelector("small")?.getAttribute("title")).toBe("Cut 3.28");
    expect(command.dataset.searchText).toBe("gabber-studio calm-otter-4 Release train Cut 3.28 building");
  });

  it("omits a missing project rather than printing a placeholder, and escapes every value", () => {
    const command = row(sessionJumpCommandMarkup({ id: 'a"b', label: "<Atlas>" }, { name: "s<1>", supervisor: 'x"y' }));
    expect(command.querySelector("small")?.textContent).toBe("<Atlas>");
    expect(command.dataset.searchText).toBe('x"y');
    expect(command.dataset.paletteMachine).toBe('a"b');
    expect(command.querySelector("span")?.textContent).toBe("Jump to s<1>");
    expect(command.querySelectorAll("*")).toHaveLength(2);
  });
});
