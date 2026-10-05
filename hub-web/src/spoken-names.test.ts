// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { renderAttachmentSheet } from "./attachment-sheet";
import { conversationRowSpokenName, type ConversationRow } from "./conversation-list";
import { countdownLabel, nextCountdown } from "./pair-dialog-markup";
import { joinSpoken, spokenSupervisor, supervisorDescription } from "./spoken-names";

describe("spoken names in plain words (cas-d8a5, journey F32)", () => {
  it("names a supervisor by its project, with the codename as the description", () => {
    expect(spokenSupervisor("cas-src", "patient-pelican-9")).toBe("cas-src supervisor");
    expect(spokenSupervisor(undefined, "patient-pelican-9")).toBe("patient-pelican-9");
    expect(supervisorDescription("cas-src", "patient-pelican-9")).toBe("patient-pelican-9");
    expect(supervisorDescription("cas-src", "wise-lion-31", true)).toBe("earlier session wise-lion-31");
    expect(supervisorDescription(undefined, "patient-pelican-9")).toBeUndefined();
  });

  it("joins parts with ', ' only after text that does not end a sentence, never leaving an empty part", () => {
    expect(joinSpoken(["Post the notes when it's out.", "11 hours ago"])).toBe("Post the notes when it's out. 11 hours ago");
    expect(joinSpoken(["calm-puma-34", "most recent", "", undefined, "2 minutes ago"])).toBe("calm-puma-34, most recent, 2 minutes ago");
    expect(joinSpoken(["", "  ", false, null])).toBe("");
    expect(joinSpoken(["The bank feed reconciled overnight.", "just now"])).not.toContain(" , ");
  });

  it("speaks a file as from the project's supervisor", () => {
    const sheet = renderAttachmentSheet(document, { artifact_id: "a", name: "report.pdf", mime: "application/pdf", size_bytes: 88_064, sha256: "9f".repeat(32) }, "the cas-src supervisor");
    expect(sheet.getAttribute("aria-label")).toBe("report.pdf, PDF, 86 KB, from the cas-src supervisor. Open");
  });

  it("speaks a grouped row without a run-on badge or a stray ' , '", () => {
    const row = {
      key: "atlas:gabber-studio-calm-puma-34", machineId: "atlas", session: "gabber-studio-calm-puma-34", supervisor: "calm-puma-34",
      projectDir: "/projects/gabber-studio", host: "Atlas · Linux", activityAt: 0, canEnd: false, freshness: "", when: "2m", whenSpoken: "2 minutes ago",
      preview: "Messaged worker.", activityLine: "", unreachable: false, connection: "Live", interrupted: false, attention: 1, unread: 2, selected: false,
      group: { active: true },
    } as unknown as ConversationRow;
    const spoken = conversationRowSpokenName(row);
    expect(spoken).toBe("gabber-studio on Atlas, calm-puma-34, most recent, Messaged worker. 2 minutes ago, 2 unread, waiting for you");
    expect(spoken).not.toMatch(/ , |calm-puma-34Most/);
  });
});

describe("the pairing countdown only goes down (cas-d8a5, journey F31)", () => {
  it("never shows more than it last showed for the request, and formats m:ss", () => {
    const expires = "2026-10-03T12:10:00Z";
    const now = Date.parse("2026-10-03T12:00:00.500Z");
    expect(countdownLabel(nextCountdown(expires, now, undefined))).toBe("9:59");
    // A rebuilt dialog, or a claim that brings a later expires_at, cannot raise it.
    expect(nextCountdown("2026-10-03T12:20:00Z", now, 599_000)).toBe(599_000);
    expect(countdownLabel(nextCountdown(expires, now + 1_000, 599_500))).toBe("9:58");
    expect(countdownLabel(0)).toBe("0:00");
    expect(countdownLabel(-5)).toBe("0:00");
  });
});
