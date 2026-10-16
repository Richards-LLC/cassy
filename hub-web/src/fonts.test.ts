// cas-4427: Commander ships its own Inter and JetBrains Mono. tokens.css names
// them first in --font-ui and --font-mono; without bundled faces every host
// rendered its own fallback (DejaVu on hosted CI, Noto Sans on the supervisor
// host), and journeys failed on font metrics alone.
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const src = (path: string) => new URL(path, import.meta.url);
const css = () => readFileSync(src("./fonts.css"), "utf8");
const faces = () => [...css().matchAll(/@font-face\s*{([^}]*)}/g)].map((match) => match[1]!);
const prop = (face: string, name: string) => new RegExp(`${name}:\\s*([^;]+);`).exec(face)?.[1]?.trim();

describe("bundled UI and mono fonts", () => {
  it("declares Inter and JetBrains Mono as the families the tokens name, self-hosted with font-display: swap", () => {
    const tokens = readFileSync(src("./tokens.css"), "utf8");
    expect(tokens).toMatch(/--font-ui: Inter,/);
    expect(tokens).toMatch(/--font-mono: "JetBrains Mono",/);
    const declared = faces();
    expect(declared.map((face) => prop(face, "font-family"))).toEqual(["Inter", '"JetBrains Mono"']);
    for (const face of declared) {
      expect(prop(face, "font-display"), face).toBe("swap");
      expect(prop(face, "font-style"), face).toBe("normal");
      expect(prop(face, "font-weight"), face).toMatch(/^100 [89]00$/);
      expect(prop(face, "unicode-range"), face).toMatch(/^U\+0000-00FF,/);
      const url = /url\("\.\/fonts\/([^"]+\.woff2)"\) format\("woff2"\)/.exec(prop(face, "src") ?? "")?.[1];
      expect(url, face).toBeTruthy();
      const bytes = readFileSync(src(`./fonts/${url}`));
      expect(bytes.subarray(0, 4).toString("latin1"), url).toBe("wOF2");
    }
    // Nothing but the bundled files: no remote font origin.
    expect(css()).not.toMatch(/https?:/);
  });

  it("pins the bundled files and keeps their OFL licenses beside them", () => {
    const pinned = [
      ["Inter-latin-wght.woff2", 48256],
      ["JetBrainsMono-latin-wght.woff2", 40404],
    ] as const;
    for (const [file, size] of pinned) {
      expect(readFileSync(src(`./fonts/${file}`)).byteLength, file).toBe(size);
    }
    for (const license of ["LICENSE.Inter.txt", "LICENSE.JetBrainsMono.txt"]) {
      expect(existsSync(src(`./fonts/${license}`)), license).toBe(true);
      expect(readFileSync(src(`./fonts/${license}`), "utf8"), license).toContain("SIL Open Font License, Version 1.1");
    }
  });
});
