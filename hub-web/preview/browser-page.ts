// A browser-side stand-in for the few Playwright Page APIs the journey
// HubDouble uses (route, routeWebSocket, addInitScript, evaluate), so the real
// Commander bundle can run against fixture machines in a plain browser tab.
// Preview only: never imported by src/ and never shipped in hub-web/dist.

type Fulfill = { status?: number; json?: unknown; body?: string; headers?: Record<string, string>; contentType?: string };

export class PreviewRoute {
  private settled?: (value: Response | Error) => void;
  readonly done: Promise<Response | Error>;
  constructor(private readonly url: string, private readonly init: RequestInit | undefined, private readonly bodyText: string | undefined) {
    this.done = new Promise((ok) => { this.settled = ok; });
  }
  request() {
    const headers: Record<string, string> = {};
    new Headers(this.init?.headers).forEach((value, key) => { headers[key.toLowerCase()] = value; });
    if (!headers.origin) headers.origin = location.origin;
    const bodyText = this.bodyText;
    return {
      url: () => this.url,
      method: () => (this.init?.method ?? "GET").toUpperCase(),
      headers: () => headers,
      postData: () => bodyText ?? null,
      postDataJSON: () => (bodyText ? JSON.parse(bodyText) : null),
      frame: () => ({ url: () => location.href }),
    };
  }
  async fulfill(options: Fulfill): Promise<void> {
    const status = options.status ?? 200;
    const headers = new Headers(options.headers);
    let body: string | null = options.body ?? null;
    if (options.json !== undefined) { body = JSON.stringify(options.json); headers.set("content-type", "application/json"); }
    if (status === 204 || status === 304) body = null;
    this.settled?.(new Response(body, { status, headers }));
  }
  async abort(reason = "failed"): Promise<void> { this.settled?.(new TypeError(`Failed to fetch (${reason})`)); }
  async continue(): Promise<void> { this.settled?.(new TypeError("preview: continue is not supported")); }
}

type Handler = (route: PreviewRoute) => unknown;

/** The route side of a socket, shaped like Playwright's WebSocketRoute. */
export class PreviewSocketRoute {
  private listeners: Array<(data: string) => void> = [];
  constructor(private readonly socket: PreviewSocket) {}
  url() { return this.socket.url; }
  onMessage(cb: (data: string) => void) { this.listeners.push(cb); }
  onClose(_cb: () => void) { /* not needed by the double */ }
  send(text: string) { this.socket.deliver(text); }
  async close(options: { code?: number; reason?: string } = {}) { this.socket.serverClose(options.code ?? 1000, options.reason ?? ""); }
  /** From the page: a frame the app wrote. */
  receive(data: string) { for (const cb of this.listeners) cb(data); }
}

const NativeSocket = window.WebSocket;

export class PreviewSocket extends EventTarget {
  static readonly CONNECTING = 0; static readonly OPEN = 1; static readonly CLOSING = 2; static readonly CLOSED = 3;
  readonly CONNECTING = 0; readonly OPEN = 1; readonly CLOSING = 2; readonly CLOSED = 3;
  readyState = 0;
  binaryType: BinaryType = "blob";
  bufferedAmount = 0;
  extensions = ""; protocol = "";
  onopen: ((e: Event) => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  readonly route: PreviewSocketRoute;
  constructor(readonly url: string) {
    super();
    this.route = new PreviewSocketRoute(this);
  }
  private fire(event: Event) {
    const handler = (this as unknown as Record<string, unknown>)[`on${event.type}`];
    if (typeof handler === "function") (handler as (e: Event) => void).call(this, event);
    this.dispatchEvent(event);
  }
  /** Mark open before the double's handler runs, so its welcome frames queue. */
  accept() { if (this.readyState === 0) this.readyState = 1; }
  /** Then announce open; queued frames follow on later ticks. */
  announceOpen() { if (this.readyState === 1) this.fire(new Event("open")); }
  send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
    if (this.readyState !== 1) throw new DOMException("WebSocket is not open", "InvalidStateError");
    const text = typeof data === "string" ? data : new TextDecoder().decode(data as ArrayBuffer);
    queueMicrotask(() => this.route.receive(text));
  }
  deliver(text: string) {
    if (this.readyState !== 1) return;
    setTimeout(() => { if (this.readyState === 1) this.fire(new MessageEvent("message", { data: text })); }, 0);
  }
  serverClose(code: number, reason: string) {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    setTimeout(() => this.fire(new CloseEvent("close", { code, reason, wasClean: code === 1000 })), 0);
  }
  close(code = 1000, reason = "") {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    setTimeout(() => this.fire(new CloseEvent("close", { code, reason, wasClean: true })), 0);
  }
}

function globMatch(pattern: string, url: string): boolean {
  const escaped = pattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*\*/g, "\u0000").replace(/\*/g, "[^/]*").replace(/\u0000/g, ".*");
  return new RegExp(`^${escaped}$`).test(url);
}

/** Minimal Page: enough for HubDouble.install, seedPaired and the push helpers. */
export class PreviewPage {
  private routes: Array<{ pattern: string; handler: Handler }> = [];
  private socketRoutes: Array<{ pattern: RegExp; handler: (ws: PreviewSocketRoute) => unknown }> = [];

  /** Install the routed fetch and socket constructor. Call before HubDouble.install. */
  installTransport(): void {
    const native = window.fetch.bind(window);
    let fetches = 0;
    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input instanceof Request ? input.url : input);
      if (++fetches > 400) throw new Error(`preview guard: fetch storm ${url}`);
      const absolute = new URL(url, location.href).href;
      const match = this.routes.find((r) => globMatch(r.pattern, absolute));
      if (!match) return native(input, init);
      let bodyText: string | undefined;
      if (input instanceof Request && !init?.body) bodyText = await input.clone().text();
      else if (typeof init?.body === "string") bodyText = init.body;
      const mergedInit: RequestInit = { method: input instanceof Request ? input.method : undefined, headers: input instanceof Request ? input.headers : undefined, ...init };
      const route = new PreviewRoute(absolute, mergedInit, bodyText);
      await match.handler(route);
      const outcome = await route.done;
      if (outcome instanceof Error) throw outcome;
      return outcome;
    };
    const page = this;
    let sockets = 0;
    const Routed = function (this: unknown, url: string | URL, protocols?: string | string[]) {
      const href = new URL(String(url), location.href).href;
      if (++sockets > 200) throw new Error(`preview guard: socket storm ${href}`);
      const match = page.socketRoutes.find((r) => r.pattern.test(href));
      if (!match) return new NativeSocket(url, protocols);
      const socket = new PreviewSocket(href);
      setTimeout(() => {
        socket.accept();
        void match.handler(socket.route);
        socket.announceOpen();
      }, 0);
      return socket;
    } as unknown as typeof WebSocket;
    Object.assign(Routed, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3, prototype: PreviewSocket.prototype });
    window.WebSocket = Routed;
  }

  async addInitScript(script: () => void): Promise<void> { script(); }
  async evaluate<T, A>(fn: (arg: A) => T | Promise<T>, arg?: A): Promise<T> { return fn(arg as A); }
  async route(pattern: string, handler: Handler): Promise<void> { this.routes.push({ pattern, handler }); }
  async routeWebSocket(pattern: RegExp, handler: (ws: PreviewSocketRoute) => unknown): Promise<void> { this.socketRoutes.push({ pattern, handler }); }
  on(): void { /* pageerror listeners are not needed in a preview */ }
}
