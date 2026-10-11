// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { AuthenticationError, HubRequestError } from "./connection";
import { WriteGrantState, sendWriteGrant, type WriteGrantSend } from "./write-grant";
import { writeAccessPanel, type WriteAccessHandlers } from "./write-grant-view";
import { presentFleetSheet } from "./fleet-sheet";
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
    const body = state.grantBody();
    await sendWriteGrant(state, send, "grant");
    expect(send).toHaveBeenCalledWith(body);
    expect(state.stage).toBe("editing");
    expect(state.result).toEqual({
      tone: "ok",
      text: "Write access granted for cas-1169: /home/u/soundwave-config/docs/requests (create+edit) until the task closes.",
      task: "cas-1169",
    });

    const revoke = filled();
    revoke.askRevoke();
    expect(revoke.stage).toBe("confirm-revoke");
    expect(revoke.question()).toBe("Revoke every write grant for cas-1169?");
    await sendWriteGrant(revoke, vi.fn<WriteGrantSend>().mockResolvedValue({ removed: 2 }), "revoke");
    expect(revoke.result).toEqual({ tone: "ok", text: "Write access revoked for cas-1169: 2 grants removed.", task: "cas-1169" });

    const refused = filled();
    refused.review();
    const error = new HubRequestError("refused", 400, "invalid_write_grant", "task cas-1169 is closed; a grant ends when its task closes");
    await sendWriteGrant(refused, vi.fn<WriteGrantSend>().mockRejectedValue(error), "grant");
    expect(refused.result).toEqual({ tone: "error", text: "Could not grant: task cas-1169 is closed; a grant ends when its task closes" });
    const forbidden = filled();
    forbidden.review();
    await sendWriteGrant(forbidden, vi.fn<WriteGrantSend>().mockRejectedValue(new HubRequestError("no", 403, "scope_denied")), "grant");
    expect(forbidden.result?.text).toBe("Could not grant: this pairing lacks the Stop and restart workers and sessions permission. Add it in Paired machines.");
  });

  // cas-42c0: the connection turns a 403 into AuthenticationError
  // ("scope-mismatch"); that is a permission refusal, not a lost pairing.
  it("says a scope refusal is about permission, and only a lost pairing asks to pair again", async () => {
    const scope = filled();
    scope.review();
    await sendWriteGrant(scope, vi.fn<WriteGrantSend>().mockRejectedValue(new AuthenticationError("scope-mismatch", "scope denied")), "grant");
    expect(scope.result).toEqual({ tone: "error", text: "Could not grant: this pairing lacks the Stop and restart workers and sessions permission. Add it in Paired machines." });
    const revoke = filled();
    revoke.askRevoke();
    await sendWriteGrant(revoke, vi.fn<WriteGrantSend>().mockRejectedValue(new AuthenticationError("scope-mismatch", "scope denied")), "revoke");
    expect(revoke.result?.text).toBe("Could not revoke: this pairing lacks the Stop and restart workers and sessions permission. Add it in Paired machines.");
    for (const kind of ["expired", "revoked", "needs-pairing"] as const) {
      const lost = filled();
      lost.review();
      await sendWriteGrant(lost, vi.fn<WriteGrantSend>().mockRejectedValue(new AuthenticationError(kind, kind)), "grant");
      expect(lost.result?.text, kind).toBe("Could not grant: the pairing is no longer accepted. Pair the machine again.");
    }
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

  // cas-06e8: a sent grant leaves no primed form behind it.
  it("clears Folder and Reason after a grant, keeping the task and modes, so one more click cannot resend it", async () => {
    const state = filled();
    state.review();
    await sendWriteGrant(state, vi.fn<WriteGrantSend>().mockResolvedValue({ grant: { path: "/home/u/x", modes: ["create", "edit"] } }), "grant");
    expect(state.draft).toMatchObject({ task: "cas-1169", path: "", reason: "" });
    expect([...state.draft.modes]).toEqual(["create", "edit"]);
    expect(state.validate()).toBe("Enter the folder to grant, as an absolute path or ~/…");
    // A refused grant keeps what was typed, to correct and resend.
    const refused = filled();
    refused.review();
    await sendWriteGrant(refused, vi.fn<WriteGrantSend>().mockRejectedValue(new HubRequestError("no", 400, "invalid_write_grant", "task is closed")), "grant");
    expect(refused.draft).toMatchObject({ path: "~/soundwave-config/docs/requests", reason: "INGEST request files" });
  });

  // cas-06e8: the task id in the receipt never breaks at its hyphen.
  it("keeps the receipt's task id in one unbreakable span", () => {
    const state = filled();
    state.result = { tone: "ok", text: "Write access granted for cas-1169: /x (create+edit) until the task closes.", task: "cas-1169" };
    const receipt = writeAccessPanel(document, { state, tasks, on: { changed: vi.fn(), review: vi.fn(), revoke: vi.fn(), confirm: vi.fn(), cancel: vi.fn() } }).querySelector(".write-grant-result")!;
    expect(receipt.textContent).toBe("Write access granted for cas-1169: /x (create+edit) until the task closes.");
    expect(receipt.querySelector(".write-grant-id")?.textContent).toBe("cas-1169");
  });

  // cas-4cf2: a visible title that matches the accessible name, what the
  // grant does, where it acts on a phone, and a close button named for it.
  it("titles the panel, says what it does and where, and names the phone sheet's close button for it", () => {
    const on: WriteAccessHandlers = { changed: vi.fn(), review: vi.fn(), revoke: vi.fn(), confirm: vi.fn(), cancel: vi.fn() };
    const state = filled();
    const panel = writeAccessPanel(document, { state, tasks, on, where: "cas-src on Atlas" });
    const title = panel.querySelector(".write-grant-title")!;
    expect(title.textContent).toBe("Write access outside the worktree");
    expect(panel.getAttribute("aria-label")).toBe(title.textContent);
    expect(panel.querySelector(".write-grant-lead")?.textContent).toBe("Lets the agents on one task write to a folder outside their worktree until the task closes.");
    expect(panel.querySelector(".write-grant-where")?.textContent).toBe("cas-src on Atlas");
    expect(writeAccessPanel(document, { state, tasks, on }).querySelector(".write-grant-where")).toBeNull();
    state.review();
    expect(writeAccessPanel(document, { state, tasks, on }).querySelector(".write-grant-title")?.textContent).toBe("Write access outside the worktree");

    HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) { this.setAttribute("open", ""); };
    const container = document.createElement("div");
    document.body.replaceChildren(container);
    container.append(writeAccessPanel(document, { state: filled(), tasks, on, where: "cas-src on Atlas" }));
    presentFleetSheet(container, vi.fn());
    const sheet = container.querySelector("dialog.fleet-action-sheet")!;
    expect(sheet.getAttribute("aria-label")).toBe("Write access outside the worktree");
    expect(sheet.querySelector(".fleet-sheet-close")?.getAttribute("aria-label")).toBe("Close write access");
  });
});
