import type { FaucetPort, FaucetResult } from "@/ports/faucet";

interface FaucetResponse {
  minted: unknown[];
  gas_tx?: string;
}

interface FaucetErrorResponse {
  error?: string;
}

const faucetOrigin = (
  import.meta.env.VITE_FAUCET_BASE_URL ?? "http://127.0.0.1:8081"
).replace(/\/+$/, "");

async function responseBody<T>(response: Response): Promise<T | undefined> {
  try {
    return (await response.json()) as T;
  } catch {
    return undefined;
  }
}

export const faucetAdapter: FaucetPort = {
  async fund(address): Promise<FaucetResult> {
    const response = await fetch(`${faucetOrigin}/faucet`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ address }),
    });

    if (!response.ok) {
      const body = await responseBody<FaucetErrorResponse>(response);
      throw new Error(
        body?.error ?? `Faucet request failed (${response.status})`,
      );
    }

    const body = await responseBody<FaucetResponse>(response);
    if (!body || !Array.isArray(body.minted))
      throw new Error("Faucet returned an invalid response");

    return {
      tokenCount: body.minted.length,
      gasFunded: body.gas_tx !== undefined,
    };
  },
};
