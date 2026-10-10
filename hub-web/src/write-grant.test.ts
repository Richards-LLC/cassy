// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { HubRequestError } from "./connection";
import { WriteGrantState, sendWriteGrant, type WriteGrantSend } from "./write-grant";
import { writeAccessPanel, type WriteAccessHandlers } from "./write-grant-view";
import type { FleetTask } from "./fleet-ops";

const tasks: FleetTask[] = [
  { id: "cas-1169", title: "Ingest request files", status: "in_progress", assignee: "swift-lark-3" },
  { id: "cas-2001", title: "Docs lane", status: "open", assignee: null },
];

function filled(): WriteGrantState {
  const state = new WriteGrantState();
  state.reset(tasks);
  state.draft.task = "cas-1169";
  state.draft.path = "~/soundwave-config/docs/requests";
  state.draft.reason = "INGEST request files";
  return state;
}

describe("Commander write grants (cas-ab04, GH #1169)", () => {
  it("starts on the first task with create+edit and asks for what is missing", () => {
    const state = new WriteGrantState();
    state.reset(tasks);
    expect(state.draft.task).toBe("cas-1169");
    expect([...state.draft.modes]).toEqual(["create", "edit"]);
    expect(state.validate()).toBe("Enter the folder to grant, as an absolute path or ~/…");
    state.draft.path = "relative/dir";
    expect(state.validate()).toBe("Enter the folder to grant, as an absolute path or ~/…");
    state.draft.path = "~/x";
    expect(state.validate()).toBe("Say why: the reason is recorded on the task.");
    state.draft.reason = "r";
    state.draft.modes.clear();
    expect(state.validate()).toBe("Choose at least one of create, edit or delete.");
  });

  it("reviews a grant, confirms, and sends the hub's body", () => {
    const state = filled();
    state.draft.modes.add("delete");
    expect(state.review()).toBe(true);
    expect(state.stage).toBe("confirm-grant");
    expect(state.question()).toBe(
      "Grant agents on cas-1169 create+edit+delete in ~/soundwave-config/docs/requests until the task closes?",
    );
    expect(state.grantBody()).toEqual({
      action: "grant",
      task: "cas-1169",
      path: "~/soundwave-config/docs/requests",
      mode: "create+edit+delete",
      reason: "INGEST request files",
    });
    expect(state.revokeBody()).toEqual({ action: "revoke", task: "cas-1169" });
  });

  it("does not open the confirmation for an incomplete grant", () => {
    const state = new WriteGrantState();
    state.reset(tasks);
    expect(state.review()).toBe(false);
    expect(state.stage).toBe("editing");
    expect(state.result).toEqual({ tone: "error", text: "Enter the folder to grant, as an absolute path or ~/…" });
  });

  it("records a receipt from the hub's answer, and the hub's refusal in words", async () => {
    const state = filled();
    state.review();
    const send = vi.fn<WriteGrantSend>().mockResolvedValue({
      grant: { task: "cas-1169", path: "/home/u/soundwave-config/docs/requests", modes: ["create", "edit"], granted_by: "commander-device:dev-1" },
    });
    await sendWriteGrant(state, send, "grant");
    expect(send).toHaveBeenCalledWith(state.grantBody());
    expect(state.stage).toBe("editing");
    expect(state.result).toEqual({
      tone: "ok",
      text: "Write access granted for cas-1169: /home/u/soundwave-config/docs/requests (create+edit) until the task closes.",
    });

    const revoke = filled();
    revoke.askRevoke();
    expect(revoke.stage).toBe("confirm-revoke");
    expect(revoke.question()).toBe("Revoke every write grant for cas-1169?");
    await sendWriteGrant(revoke, vi.fn<WriteGrantSend>().mockResolvedValue({ removed: 2 }), "revoke");
    expect(revoke.result).toEqual({ tone: "ok", text: "Write access revoked for cas-1169: 2 grants removed." });

    const refused = filled();
    refused.review();
    const error = new HubRequestError("refused", 400, "invalid_write_grant", "task cas-1169 is closed; a grant ends when its task closes");
    await sendWriteGrant(refused, vi.fn<WriteGrantSend>().mockRejectedValue(error), "grant");
    expect(refused.result).toEqual({ tone: "error", text: "Could not grant: task cas-1169 is closed; a grant ends when its task closes" });
    const forbidden = filled();
    forbidden.review();
    await sendWriteGrant(forbidden, vi.fn<WriteGrantSend>().mockRejectedValue(new HubRequestError("no", 403, "scope_denied")), "grant");
    expect(forbidden.result?.text).toBe("Could not grant: this pairing does not allow managing workers. Add it in Paired machines.");
  });

  it("renders the form, the confirmation and the receipt", () => {
    const handlers: WriteAccessHandlers = { changed: vi.fn(), review: vi.fn(), revoke: vi.fn(), confirm: vi.fn(), cancel: vi.fn() };
    const state = filled();
    let panel = writeAccessPanel(document, { state, tasks, on: handlers });
    const task = panel.querySelector<HTMLSelectElement>("select.write-grant-task")!;
    expect([...task.options].map((option) => option.value)).toEqual(["cas-1169", "cas-2001"]);
    const path = panel.querySelector<HTMLInputElement>("input.write-grant-path")!;
    expect(path.value).toBe("~/soundwave-config/docs/requests");
    path.value = "~/other";
    path.dispatchEvent(new Event("input"));
    expect(state.draft.path).toBe("~/other");
    expect(handlers.changed).toHaveBeenCalled();
    const modes = [...panel.querySelectorAll<HTMLInputElement>("input.write-grant-mode")];
    expect(modes.map((mode) => [mode.value, mode.checked])).toEqual([["create", true], ["edit", true], ["delete", false]]);
    panel.querySelector<HTMLButtonElement>("button.write-grant-review")!.click();
    expect(handlers.review).toHaveBeenCalled();

    state.review();
    panel = writeAccessPanel(document, { state, tasks, on: handlers });
    const confirm = panel.querySelector(".write-grant-confirm")!;
    expect(confirm.getAttribute("role")).toBe("alertdialog");
    expect(confirm.textContent).toContain("Grant agents on cas-1169 create+edit in ~/other until the task closes?");
    const buttons = [...confirm.querySelectorAll("button")].map((button) => button.textContent);
    expect(buttons).toEqual(["Cancel", "Grant"]);

    state.stage = "editing";
    state.result = { tone: "ok", text: "Write access granted for cas-1169: /x (create+edit) until the task closes." };
    panel = writeAccessPanel(document, { state, tasks, on: handlers });
    const receipt = panel.querySelector(".write-grant-result")!;
    expect(receipt.getAttribute("role")).toBe("status");
    expect(receipt.textContent).toContain("Write access granted for cas-1169");
  });
});
