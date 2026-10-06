// cas-9b7d: @hpke/common falls back to `import("crypto")` (Node's webcrypto)
// when `globalThis.crypto` is missing. Commander only runs where WebCrypto
// exists, and the hub serves a fixed dist file list (cas-cli/src/hub/server.rs
// include_bytes), so that fallback must not become an extra bundle chunk.
// Vite aliases `crypto` here; reaching it means the browser lacks WebCrypto.
export const webcrypto = undefined;
export default {};
