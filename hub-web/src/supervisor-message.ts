import type { HubSession } from "./types";

export function supervisorTarget(session: HubSession | undefined): string | undefined {
  const target = session?.supervisor.trim();
  return target || undefined;
}

export function supervisorMessage(target: string, text: string): Record<string, unknown> {
  return {
    SendMessage: {
      target,
      text,
      summary: "Cassy Commander message",
      urgent: false,
      attribution: {
        device_id: null,
        credential_id: null,
        device_label: null,
        operator_label: null,
        controller_origin: null,
        request_id: null,
      },
    },
  };
}
