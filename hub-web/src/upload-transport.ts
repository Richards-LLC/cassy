/** Upload byte counts need XMLHttpRequest; fetch exposes no portable upload events.
 * Signing, refusal parsing, auth retries and the deadline stay with the connection. */
export function uploadRequest(url: URL, init: RequestInit, report: (fraction: number) => void): Promise<Response> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const signal = init.signal;
    const cleanup = () => signal?.removeEventListener("abort", abort);
    const abort = () => { xhr.abort(); cleanup(); reject(signal?.reason ?? new DOMException("Upload cancelled", "AbortError")); };
    xhr.open(init.method ?? "POST", url);
    new Headers(init.headers).forEach((value, name) => xhr.setRequestHeader(name, value));
    xhr.upload.onprogress = event => { if (event.lengthComputable && event.total > 0) report(event.loaded / event.total); };
    xhr.upload.onload = () => report(1);
    xhr.onload = () => {
      cleanup();
      const headers = new Headers();
      for (const line of xhr.getAllResponseHeaders().trim().split(/[\r\n]+/)) {
        const colon = line.indexOf(":");
        if (colon > 0) headers.append(line.slice(0, colon), line.slice(colon + 1).trim());
      }
      resolve(new Response(xhr.status === 204 ? null : xhr.responseText, { status: xhr.status, statusText: xhr.statusText, headers }));
    };
    xhr.onerror = () => { cleanup(); reject(new TypeError("The upload connection was interrupted. Try sending again.")); };
    xhr.onabort = () => { cleanup(); reject(signal?.reason ?? new DOMException("Upload cancelled", "AbortError")); };
    signal?.addEventListener("abort", abort, { once: true });
    if (signal?.aborted) { abort(); return; }
    report(0);
    xhr.send(init.body as string);
  });
}
