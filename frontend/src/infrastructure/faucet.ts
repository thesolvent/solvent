import type { Address } from "@/domain";
import type { Faucet } from "@/ports/faucet";

import { env } from "./env";

/** The devnet faucet client: `POST /faucet { address }`, resolving on a receipt-verified mint. */
export function createFaucet(baseUrl: string): Faucet {
  const base = baseUrl.replace(/\/$/, "");
  return {
    async requestTokens(address: Address): Promise<void> {
      let res: Response;
      try {
        res = await fetch(`${base}/faucet`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ address }),
        });
      } catch (cause) {
        throw new Error("faucet is unreachable", { cause });
      }
      if (!res.ok) {
        const detail = await res.text().catch(() => "");
        throw new Error(detail || `faucet request failed (${res.status})`);
      }
    },
  };
}

export const faucet: Faucet = createFaucet(env.faucetUrl);
