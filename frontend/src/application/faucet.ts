import { useMutation } from "@tanstack/react-query";

import type { Address } from "@/domain";

import { useFaucetClient } from "./services";

/** Devnet "Get test tokens": mint the seeded assets to an address. */
export function useRequestTokens() {
  const faucet = useFaucetClient();
  return useMutation({
    mutationFn: (address: Address) => faucet.requestTokens(address),
  });
}
