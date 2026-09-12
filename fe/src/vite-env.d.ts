/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Origin of the Solvent API (`/v1/*`). Defaults to the local devnet server. */
  readonly VITE_API_BASE_URL?: string;
  /** Origin of the local devnet faucet. */
  readonly VITE_FAUCET_BASE_URL?: string;
  /** Origin of the destination index scoped to the direct settlement app. */
  readonly VITE_SOLVENTX_DIRECT_API_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
