import { describe, expect, it, vi } from "vitest";
import { HubConnectionSupervisor, type HubCallbacks } from "./connection";
import type { StoredMachine } from "./types";

describe("Commander MessageQueued callback", () => {
  it("relays the daemon acknowledgment with its client reference", () => {
    const onMessageQueued = vi.fn();
    const callbacks = { onMessageQueued } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor(Object.assign({ credentialId: "send-credential" } as StoredMachine, { credentialGeneration: 7 }), callbacks);
    const internals = supervisor as unknown as {
      handleDaemonObject(session: string, message: Record<string, unknown>): void;
    };

    internals.handleDaemonObject("factory-a", {
      MessageQueued: {
        client_ref: "send-42",
        notification_id: 812,
        target: "patient-pelican-9",
        stamped: true,
        device_label: "Desktop",
      },
    });

    expect(onMessageQueued).toHaveBeenCalledWith("factory-a", {
      client_ref: "send-42",
      notification_id: 812,
      target: "patient-pelican-9",
      stamped: true,
      device_label: "Desktop",
    }, { credentialId: "send-credential", generation: 7 });
  });
});

describe("Commander live operator sends", () => {
  it("dispatches a durable send before its supervisor reply", () => {
    const order: string[] = [];
    const callbacks = {
      onOperatorMessage: (_session: string, message: { text: string }) => order.push(message.text),
      onOperatorReply: (_session: string, reply: { message: string }) => order.push(reply.message),
    } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor({} as StoredMachine, callbacks);
    const internals = supervisor as unknown as {
      handleDaemonObject(session: string, message: Record<string, unknown>): void;
    };
    internals.handleDaemonObject("factory-a", { OperatorMessage: {
      notification_id: 41, target: "supervisor", text: "Question", state: "sending",
      stamped: true, device_id: "phone", operator_label: "Pixel 10", at: "2026-09-29T16:00:00Z",
    } });
    internals.handleDaemonObject("factory-a", { OperatorReply: {
      notification_id: 42, reply_to: 41, message: "Answer", summary: "", device_id: "phone",
    } });
    expect(order).toEqual(["Question", "Answer"]);
  });
});

describe("Commander rejected-message callback", () => {
  it("correlates the legacy refusal with the submitted client reference", () => {
    const onMessageRejected = vi.fn();
    const onSocketError = vi.fn();
    const callbacks = { onMessageRejected, onSocketError } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor({} as StoredMachine, callbacks);
    const internals = supervisor as unknown as {
      handleDaemonObject(session: string, message: Record<string, unknown>): void;
    };

    internals.handleDaemonObject("factory-a", { error: "forbidden", client_ref: "send-42" });

    expect(onMessageRejected).toHaveBeenCalledWith("factory-a", "send-42", "forbidden", { code: "forbidden", retryable: false });
    expect(onSocketError).not.toHaveBeenCalled();
  });

  it("correlates the multiplex refusal without treating it as a transport failure", async () => {
    const onMessageRejected = vi.fn();
    const onSocketError = vi.fn();
    const callbacks = { onMessageRejected, onSocketError } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor({} as StoredMachine, callbacks);
    const internals = supervisor as unknown as {
      handleMachineMessage(input: string): Promise<void>;
    };

    await internals.handleMachineMessage(JSON.stringify({
      channel: "pty:factory-a",
      error: { code: "forbidden", client_ref: "send-42" },
    }));

    expect(onMessageRejected).toHaveBeenCalledWith("factory-a", "send-42", "forbidden", { code: "forbidden", retryable: false });
    expect(onSocketError).not.toHaveBeenCalled();
  });

  it("keeps uncorrelated legacy errors on the socket-error path", () => {
    const onMessageRejected = vi.fn();
    const onSocketError = vi.fn();
    const callbacks = { onMessageRejected, onSocketError } as unknown as HubCallbacks;
    const supervisor = new HubConnectionSupervisor({} as StoredMachine, callbacks);
    const internals = supervisor as unknown as {
      handleDaemonObject(session: string, message: Record<string, unknown>): void;
    };

    internals.handleDaemonObject("factory-a", { error: "forbidden" });

    expect(onMessageRejected).not.toHaveBeenCalled();
    expect(onSocketError).toHaveBeenCalledWith("factory-a", "forbidden");
  });
});
