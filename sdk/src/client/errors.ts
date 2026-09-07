/** A non-2xx API response, or a 2xx envelope that carries an `error`. */
export class SolventApiError extends Error {
  readonly status: number;
  readonly body?: string;

  constructor(status: number, message: string, body?: string) {
    super(message);
    this.name = "SolventApiError";
    this.status = status;
    this.body = body;
  }
}

/** The transport itself failed (network down, DNS, aborted) — no HTTP response arrived. */
export class SolventNetworkError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "SolventNetworkError";
  }
}
