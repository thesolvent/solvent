import { describe, expect, it } from "vitest";

import {
  createSolventClient,
  SolventApiError,
  SolventNetworkError,
  type Transport,
} from "../../src/client/index";

/** A transport that returns a 200 envelope wrapping `result`, capturing the URL it was called with. */
function okTransport(result: unknown, onUrl?: (url: string) => void): Transport {
  return async (url) => {
    onUrl?.(url);
    return new Response(JSON.stringify({ status: 200, result }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
}

describe("createSolventClient", () => {
  it("unwraps the envelope result", async () => {
    const client = createSolventClient({
      baseUrl: "https://x.test",
      transport: okTransport({ chain_id: 31337 }),
    });
    const cfg = await client.config();
    expect(cfg.chain_id).toBe(31337);
  });

  it("returns list pages and puts query params in the URL", async () => {
    let seen = "";
    const client = createSolventClient({
      baseUrl: "https://x.test/",
      transport: okTransport({ items: [{}] }, (url) => (seen = url)),
    });
    const pairs = await client.pairs({ search: "weth" });
    expect(pairs.items).toHaveLength(1);
    expect(seen).toBe("https://x.test/v1/pairs?search=weth");
  });

  it("throws SolventApiError on an error envelope", async () => {
    const transport: Transport = async () =>
      new Response(JSON.stringify({ status: 422, error: "no route" }), { status: 422 });
    const client = createSolventClient({ baseUrl: "https://x.test", transport });
    await expect(
      client.quote({ token_in: "0x", token_out: "0x", amount_in: "1" }),
    ).rejects.toMatchObject({ name: "SolventApiError", status: 422, message: "no route" });
  });

  it("throws SolventNetworkError when the transport fails", async () => {
    const transport: Transport = async () => {
      throw new Error("offline");
    };
    const client = createSolventClient({ baseUrl: "https://x.test", transport });
    await expect(client.stats()).rejects.toBeInstanceOf(SolventNetworkError);
  });
});
