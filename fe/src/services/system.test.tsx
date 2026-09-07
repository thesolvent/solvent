import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { AppConfig } from "@solvent/sdk/client";

import { renderWithServices } from "@/test/harness";

import { useConfig } from "./system";

const CONFIG: AppConfig = {
  block_explorer_url: "http://localhost:5100",
  chain_id: 31337,
  default_fee_bps: 5,
  features: { earn: true, faucet: true, send_buy: true },
  networks: ["Ethereum"],
};

function Probe() {
  const { data, isPending, isError } = useConfig();
  if (isError) return <p>failed</p>;
  if (isPending) return <p>loading</p>;
  return <p>chain {data?.chain_id}</p>;
}

describe("useConfig", () => {
  it("resolves the server config through the injected API port", async () => {
    const config = vi.fn().mockResolvedValue(CONFIG);
    renderWithServices(<Probe />, { config });

    expect(await screen.findByText("chain 31337")).toBeInTheDocument();
    expect(config).toHaveBeenCalledOnce();
  });

  it("reports a failed read rather than leaving the caller pending", async () => {
    const config = vi.fn().mockRejectedValue(new Error("api down"));
    renderWithServices(<Probe />, { config });

    expect(await screen.findByText("failed")).toBeInTheDocument();
  });
});
