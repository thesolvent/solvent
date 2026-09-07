/** A `fetch`-shaped function — the SDK's single I/O seam. Wrap it to add auth, retries, or a mock
 * in tests; it defaults to the platform `fetch`. */
export type Transport = (input: string, init?: RequestInit) => Promise<Response>;

/** The platform `fetch` (native in Node 18+ and browsers). */
export const defaultTransport: Transport = (input, init) => globalThis.fetch(input, init);
