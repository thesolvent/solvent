/** Generates the two chain-local `solvent.<side>.toml` configs (+ signing env) for the two-chain
 *  deploy, wiring each side's `[crosschain]` block to the other. Mirrors
 *  `../devnet/bootstrap.ts`'s single-chain TOML shape; kept separate rather than parameterizing
 *  that file, since the single-chain flow must keep working unmodified. */
import { randomBytes } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { REPO_ROOT } from "../lib/manifest.ts";
import type { Manifest } from "../lib/manifest.ts";
import { UNISWAP_ASSETS } from "../devnet/bootstrap.ts";
import type { CrossChainInfraManifest, Side } from "./manifests.ts";

// Anvil's deterministic dev accounts — well-known throwaway keys, devnet only. Each chain is
// independent, so reusing the same account index on both sides is safe: nonces are per-chain.
// The filler owner and policy signer must be distinct from the deployer — `DeployDevnet` deploys
// `UniswapXAquaFiller`/`Erc7683AquaFiller` owned by `FILLER_OWNER_KEY`'s address and trusting
// `POLICY_SIGNER_KEY`'s address for policy-authorized fills, matching the same-chain devnet's own
// `devnet/docker-compose.yml` seed service and `scripts/src/devnet/bootstrap.ts` exactly — the
// backend must sign as these same keys, or nothing it signs verifies against the deployed fillers.
export const FILLER_OWNER_KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const POLICY_SIGNER_KEY = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";
const COSIGNER_KEY = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

const NAMES: Record<string, string> = {
  WETH: "Wrapped Ether",
  WBTC: "Wrapped BTC",
  USDC: "USD Coin",
  USDT: "Tether USD",
  DAI: "Dai Stablecoin",
  LINK: "ChainLink Token",
};
const TAGS: Record<string, string> = {
  WETH: "Majors",
  WBTC: "Majors",
  USDC: "Stables",
  USDT: "Stables",
  DAI: "Stables",
  LINK: "DeFi",
};
const PRICE_FEEDS: Record<string, string> = {
  ETHUSDT: "WETH",
  BTCUSDT: "WBTC",
  USDCUSDT: "USDC",
  LINKUSDT: "LINK",
};
const PEGGED = ["USDT", "DAI"];

export interface SidePorts {
  apiPort: number;
  crosschainPort: number;
  rpcUrl: string;
  explorerUrl: string;
}

export const COORDINATOR_PORT = 8090;

export const DIRECT_DESTINATION_PORTS: SidePorts = {
  apiPort: 8401,
  crosschainPort: 9382,
  rpcUrl: "http://127.0.0.1:9646",
  explorerUrl: "http://localhost:5301",
};

// Deliberately disjoint from both `docker-compose.yml`'s single-chain devnet (8545/5100/8081/8080)
// and the `pr14-merge-master` worktree's own crosschain deploy (9545/9546/5200/5201/9081/9082/
// 8299/8300/9280/9281) — either may be someone else's active work running alongside this one.
export const PORTS: Record<Side, SidePorts> = {
  origin: {
    apiPort: 8399,
    crosschainPort: 9380,
    rpcUrl: "http://127.0.0.1:9645",
    explorerUrl: "http://localhost:5300",
  },
  destination: {
    apiPort: 8400,
    crosschainPort: 9381,
    rpcUrl: "http://127.0.0.1:9646",
    explorerUrl: "http://localhost:5301",
  },
};

function address(manifest: Manifest, symbol: string): string {
  const token = manifest.tokens[symbol];
  if (!token) throw new Error(`manifest has no ${symbol} token`);
  return token.address;
}

function tokenList(manifest: Manifest) {
  return {
    name: "Solvent Devnet",
    version: { major: 1, minor: 0, patch: 0 },
    tokens: Object.entries(manifest.tokens).map(([symbol, token]) => ({
      chainId: manifest.chain_id,
      address: token.address,
      symbol,
      name: NAMES[symbol] ?? symbol,
      decimals: token.decimals,
      tags: [TAGS[symbol] ?? "other"],
    })),
  };
}

export interface CrossWiring {
  /** This lane's two ends — identical on both sides' config, they name the same trade lane. */
  originSettler: string;
  destinationApp: string;
  /** The shared secret the two internal listeners authenticate each other's requests with. */
  internalToken: string;
}

export interface DirectRouteConfig {
  maker: string;
  originStrategyHash: string;
  originSettler: string;
  compact: string;
  compactLockTag: string;
  destinationApp: string;
  originProofOutbox: string;
  destinationProofOutbox: string;
  destinationFillVerifier: string;
  originChainId: number;
  destinationChainId: number;
}

export interface SideConfigOptions {
  configName?: string;
  appAddress?: string;
  ports?: SidePorts;
  databaseName?: string;
  directRoute?: DirectRouteConfig;
}

export interface CoordinatorConfig {
  configPath: string;
}

function configToml(
  side: Side,
  manifest: Manifest,
  infra: CrossChainInfraManifest,
  wiring: CrossWiring,
  tokenListPath: string,
  options: SideConfigOptions,
): string {
  const ports = options.ports ?? PORTS[side];
  const appAddress = options.appAddress ?? manifest.router;
  const databaseName = options.databaseName ?? "solvent.db";
  const configName = options.configName ?? `solvent.${side}`;
  const pegs = PEGGED.filter((s) => manifest.tokens[s])
    .map((s) => `"${address(manifest, s)}"`)
    .join(", ");
  const feeds = Object.entries(PRICE_FEEDS)
    .filter(([, symbol]) => manifest.tokens[symbol])
    .map(
      ([feed, symbol]) =>
        `[[price_symbols]]\nsymbol = "${feed}"\ntokens = ["${address(manifest, symbol)}"]  # ${symbol}\n`,
    )
    .join("\n");
  const uniswapAssets = UNISWAP_ASSETS.map((asset) => {
    const market = asset.marketSymbol
      ? `market_symbol = "${asset.marketSymbol}"\n`
      : "";
    const peg = asset.usdPeg ? "usd_peg = true\n" : "";
    return `[[uniswap_assets]]
source_address = "${asset.sourceAddress}"
symbol = "${asset.symbol}"
decimals = ${asset.decimals}
${market}${peg}`;
  }).join("\n");

  return `# Generated by @solvent/scripts deploy — regenerate after every deploy, do not hand-edit.
# Side: ${side}. Addresses come from devnet/generated/crosschain/${side}/.

bind_addr = "0.0.0.0:${ports.apiPort}"
rpc_url = "${ports.rpcUrl}"
chain_id = ${manifest.chain_id}

database_url = "sqlite:devnet/generated/crosschain/${side}/${databaseName}?mode=rwc"

aqua_address = "${manifest.aqua}"
app_address = "${appAddress}"

default_fee_bps = 5
block_explorer_url = "${ports.explorerUrl}"
networks = ["Ethereum"]
faucet = true

token_list = "${tokenListPath}"

native_token = "${address(manifest, "WETH")}"
gas_units_per_leg = 150000
binance_ws_url = "wss://stream.binance.com:9443"
usd_stable_pegs = [${pegs}]

filler = "${manifest.filler}"
erc7683_settler = "${manifest.erc7683_settler}"
erc7683_filler = "${manifest.erc7683_filler}"
erc7683_resolver = "${manifest.erc7683_resolver}"
erc7683_executor_fee_bps = 5
reactor = "${manifest.reactor}"
permit2 = "${manifest.permit2}"
confirmations = 1
reservation_ttl_secs = 120
decay_window_secs = 60

wallet_state_db = "devnet/generated/crosschain/${configName}/walletkit.redb"

[rebate]
authorization_ttl_blocks = 90

${feeds}
${uniswapAssets}

# Private cross-chain service: exposed only to the other side's process (or a proxy between them
# in a real multi-host deployment). SOLVENT_INTERNAL_TOKEN below is the shared Bearer credential.
# direct_author is present only on the isolated destination direct-quoter.
[crosschain]
bind_addr = "127.0.0.1:${ports.crosschainPort}"
destination_app = "${wiring.destinationApp}"
origin_settler = "${wiring.originSettler}"
proof_outbox = "${infra.outbox}"
quote_ttl_secs = 300
${options.directRoute ? `
[crosschain.direct_author]
origin_chain_id = ${options.directRoute.originChainId}
origin_proof_outbox = "${options.directRoute.originProofOutbox}"
destination_proof_outbox = "${options.directRoute.destinationProofOutbox}"
origin_strategy_hash = "${options.directRoute.originStrategyHash}"
` : ""}`;
}

export function writeSideConfig(
  side: Side,
  manifest: Manifest,
  infra: CrossChainInfraManifest,
  wiring: CrossWiring,
  options: SideConfigOptions = {},
): { configPath: string; envPath: string } {
  const configName = options.configName ?? `solvent.${side}`;
  const dir = resolve(REPO_ROOT, "devnet/generated/crosschain", configName);
  const tokensOut = resolve(dir, "tokens.json");
  const configOut = resolve(REPO_ROOT, `${configName}.toml`);
  const envOut = resolve(dir, "env.sh");

  mkdirSync(dir, { recursive: true });
  writeFileSync(tokensOut, `${JSON.stringify(tokenList(manifest), null, 2)}\n`);
  writeFileSync(
    configOut,
    configToml(side, manifest, infra, wiring, `devnet/generated/crosschain/${configName}/tokens.json`, options),
  );
  mkdirSync(dirname(envOut), { recursive: true });
  writeFileSync(
    envOut,
    "# Signing keys + internal auth this side's server reads from the environment.\n" +
      `export SOLVENT_SIGNER_KEY=${FILLER_OWNER_KEY}\n` +
      `export SOLVENT_POLICY_SIGNER_KEY=${POLICY_SIGNER_KEY}\n` +
      `export SOLVENT_COSIGNER_KEY=${COSIGNER_KEY}\n` +
      `export SOLVENT_INTERNAL_TOKEN=${wiring.internalToken}\n` +
      (options.directRoute ? `export SOLVENT_CROSSCHAIN_MAKER_KEY=${FILLER_OWNER_KEY}\n` : ""),
  );
  console.log(`config: wrote ${configOut} + ${envOut}`);
  return { configPath: configOut, envPath: envOut };
}

export function writeCoordinatorConfig(directRoute: DirectRouteConfig): CoordinatorConfig {
  const configPath = resolve(REPO_ROOT, "devnet/generated/crosschain/proxy.toml");
  const config = `bind_addr = "127.0.0.1:${COORDINATOR_PORT}"
database_url = "sqlite:devnet/generated/crosschain/proxy.db?mode=rwc"
origin_service_url = "http://127.0.0.1:${PORTS.origin.crosschainPort}/"
destination_service_url = "http://127.0.0.1:${DIRECT_DESTINATION_PORTS.crosschainPort}/"
poll_interval_ms = 250

[direct]
origin_chain_id = ${directRoute.originChainId}
destination_chain_id = ${directRoute.destinationChainId}
origin_settler = "${directRoute.originSettler}"
compact = "${directRoute.compact}"
destination_settler = "${directRoute.destinationApp}"
fill_proof_verifier = "${directRoute.destinationFillVerifier}"
exclusive_filler = "${directRoute.maker}"
compact_lock_tag = "${directRoute.compactLockTag.slice(0, 26)}"
`;
  writeFileSync(configPath, config);
  console.log(`config: wrote ${configPath}`);
  return { configPath };
}

export function generateInternalToken(): string {
  return randomBytes(32).toString("hex");
}
