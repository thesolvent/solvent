import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { Address } from "viem";

import { useServices } from "./context";

export function useFaucet() {
  const { faucet } = useServices();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (address: Address) => faucet.fund(address),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["create-position", "pairs"],
        }),
        queryClient.invalidateQueries({ queryKey: ["makers"] }),
      ]);
    },
  });
}
