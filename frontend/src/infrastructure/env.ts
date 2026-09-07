interface Env {
  apiBaseUrl: string;
  rpcUrl: string;
  chainId: number;
  faucetUrl: string;
  walletConnectProjectId: string;
}

/** Runtime config from Vite env, with devnet defaults. */
export const env: Env = {
  apiBaseUrl: import.meta.env.VITE_API_BASE_URL ?? "http://localhost:8080",
  rpcUrl: import.meta.env.VITE_RPC_URL ?? "http://localhost:8545",
  chainId: Number(import.meta.env.VITE_CHAIN_ID ?? "31337"),
  faucetUrl: import.meta.env.VITE_FAUCET_URL ?? "http://localhost:8080",
  walletConnectProjectId:
    import.meta.env.VITE_WALLETCONNECT_PROJECT_ID ?? "solvent-devnet",
};
