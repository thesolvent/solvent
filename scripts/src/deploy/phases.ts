/** The four deploy phases, each idempotent: re-running the orchestrator after a crash re-verifies
 *  a phase's manifest against live chain state and only redeploys if that check fails (stale
 *  manifest, wrong chain, wiped state) rather than trusting the file's mere existence — the exact
 *  gap that let a stray anvil process silently invalidate a "successful" deploy earlier. */
import type { Address } from "viem";

import type { Manifest } from "../lib/manifest.ts";
import { assertDeployed, nextCreateAddress, waitForRpc } from "./chain.ts";
import { runForgeScript, toContainerPath } from "./forge.ts";
import {
  appManifestPath,
  compactManifestPath,
  devnetManifestPath,
  infraManifestPath,
  settlerManifestPath,
  tryRead,
  write,
  type CrossChainInfraManifest,
  type DestinationAppManifest,
  type OriginCompactManifest,
  type OriginSettlerManifest,
  type Side,
} from "./manifests.ts";

const DEPLOYER_KEY = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const DEPLOYER_ADDRESS = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266" as Address;
/** `DeployDevnet` requires the filler's owner and its policy signer to be distinct from the
 *  broadcasting deployer key (anvil dev accounts #2/#3) — matches the same-chain devnet's own
 *  `devnet/docker-compose.yml` seed service and `scripts/src/devnet/bootstrap.ts`. */
const FILLER_OWNER_KEY = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
const POLICY_SIGNER_KEY = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";
/** A nonzero placeholder for the CCTP wiring this devnet never actually exercises (no real
 *  message transmitter locally) — the constructors require *an* address, not a working one. */
const DEVNET_DUMMY = "0x000000000000000000000000000000000000dEaD";

/** Fixed per-chain identity numbers this local relay/CCIP-mock setup uses to route messages —
 *  arbitrary but must match `scripts/src/crosschain/relay.ts`. */
export const SELECTOR: Record<Side, bigint> = { origin: 11n, destination: 22n };

export interface ChainTarget {
  side: Side;
  /** Reachable from the host (Node's viem calls: waiting for the chain, reading nonces, verifying
   *  deployed code). */
  rpcUrl: string;
  /** Reachable from inside the `solvent-crosschain-erc7683_default` docker network (a container DNS
   *  name) — what `forge script` actually connects to, since it runs in its own container. */
  internalRpcUrl: string;
  chainId: number;
}

async function deployedManifestOrRedeploy<T>(
  label: string,
  manifestPath: string,
  rpcUrl: string,
  addressesToVerify: (m: T) => Record<string, string>,
  deploy: () => Promise<T>,
): Promise<T> {
  const existing = tryRead<T>(manifestPath);
  if (existing) {
    try {
      await assertDeployed(rpcUrl, addressesToVerify(existing));
      console.log(`${label}: already deployed and verified, skipping`);
      return existing;
    } catch (error) {
      console.log(
        `${label}: manifest present but stale (${(error as Error).message}) — redeploying`,
      );
    }
  }
  const manifest = await deploy();
  write(manifestPath, manifest);
  return manifest;
}

/** Phase 1: the same-chain stack (Aqua, router, reactor, UniswapX filler, ERC-7683 settler/filler/
 *  resolver, Core-6 tokens) on one chain. Identical to the single-chain devnet's own deploy. */
export async function deploySameChainStack(target: ChainTarget): Promise<Manifest> {
  await waitForRpc(target.rpcUrl, target.chainId);
  const manifestPath = devnetManifestPath(target.side);
  return deployedManifestOrRedeploy<Manifest>(
    `[${target.side}] same-chain stack`,
    manifestPath,
    target.rpcUrl,
    (m) => ({ aqua: m.aqua, router: m.router, filler: m.filler, reactor: m.reactor }),
    async () => {
      await runForgeScript({
        scriptPath: "script/DeployDevnet.s.sol",
        contract: "DeployDevnet",
        rpcUrl: target.internalRpcUrl,
        privateKey: DEPLOYER_KEY,
        env: {
          DEVNET_MANIFEST: toContainerPath(manifestPath),
          FILLER_OWNER_KEY,
          POLICY_SIGNER_KEY,
        },
      });
      return tryRead<Manifest>(manifestPath)!;
    },
  );
}

/** Phase 2: the CCIP-mock proof rail (router + inbox + outbox) on one chain, addressed to the
 *  *other* chain's fixed selector. */
export async function deployCrossChainInfra(target: ChainTarget): Promise<CrossChainInfraManifest> {
  const remoteSelector = SELECTOR[target.side === "origin" ? "destination" : "origin"];
  const manifestPath = infraManifestPath(target.side);
  return deployedManifestOrRedeploy<CrossChainInfraManifest>(
    `[${target.side}] crosschain infra`,
    manifestPath,
    target.rpcUrl,
    (m) => ({ router: m.router, inbox: m.inbox, outbox: m.outbox }),
    async () => {
      await runForgeScript({
        scriptPath: "script/DeployCrossChainInfra.s.sol",
        contract: "DeployCrossChainInfra",
        rpcUrl: target.internalRpcUrl,
        privateKey: DEPLOYER_KEY,
        env: {
          CCIP_REMOTE_CHAIN_SELECTOR: remoteSelector.toString(),
          CCIP_DESTINATION_CHAIN_SELECTOR: remoteSelector.toString(),
          CROSSCHAIN_MANIFEST: toContainerPath(manifestPath),
        },
      });
      return tryRead<CrossChainInfraManifest>(manifestPath)!;
    },
  );
}

/** Phase 3: the origin-side Compact settlement primitives (TheCompact + an always-allow
 *  allocator). Origin-only, destination needs nothing from this. */
export async function deployOriginCompact(origin: ChainTarget): Promise<OriginCompactManifest> {
  const manifestPath = compactManifestPath();
  return deployedManifestOrRedeploy<OriginCompactManifest>(
    "[origin] compact",
    manifestPath,
    origin.rpcUrl,
    (m) => ({ compact: m.compact, allocator: m.allocator }),
    async () => {
      await runForgeScript({
        scriptPath: "script/DeployCrossChainInfra.s.sol",
        contract: "DeployOriginCompact",
        rpcUrl: origin.internalRpcUrl,
        privateKey: DEPLOYER_KEY,
        env: { COMPACT_MANIFEST: toContainerPath(manifestPath) },
      });
      return tryRead<OriginCompactManifest>(manifestPath)!;
    },
  );
}

/** Phase 4: the two circularly-dependent contracts — `CompactOriginSettler` (origin) names
 *  `CrossChainAquaApp`'s (destination) address in its constructor, and vice versa. Both addresses
 *  are precomputed from each deployer's current nonce *before* either deploys, then verified to
 *  land exactly where predicted — the only way to break a two-sided immutable reference cycle
 *  without a redeploy-and-relink step the contracts don't support. */
export async function deployCrossWiredApps(
  origin: ChainTarget,
  destination: ChainTarget,
  originDevnet: Manifest,
  destinationDevnet: Manifest,
  originInfra: CrossChainInfraManifest,
  destinationInfra: CrossChainInfraManifest,
  originCompact: OriginCompactManifest,
): Promise<{ settler: OriginSettlerManifest; app: DestinationAppManifest }> {
  const existingSettler = tryRead<OriginSettlerManifest>(settlerManifestPath());
  const existingApp = tryRead<DestinationAppManifest>(appManifestPath());
  if (existingSettler && existingApp) {
    try {
      await assertDeployed(origin.rpcUrl, { settler: existingSettler.settler });
      await assertDeployed(destination.rpcUrl, { app: existingApp.app });
      console.log("[origin+destination] cross-wired apps: already deployed and verified, skipping");
      return { settler: existingSettler, app: existingApp };
    } catch (error) {
      console.log(
        `cross-wired apps: manifest present but stale (${(error as Error).message}) — redeploying both`,
      );
    }
  }

  const predictedSettler = await nextCreateAddress(origin.rpcUrl, DEPLOYER_ADDRESS);
  const predictedApp = await nextCreateAddress(destination.rpcUrl, DEPLOYER_ADDRESS);
  console.log(`  predicted origin settler:     ${predictedSettler}`);
  console.log(`  predicted destination app:    ${predictedApp}`);

  await runForgeScript({
    scriptPath: "script/DeployCrossChainInfra.s.sol",
    contract: "DeployDestinationCrossChainApp",
    rpcUrl: destination.internalRpcUrl,
    privateKey: DEPLOYER_KEY,
    env: {
      DEVNET_DUMMY,
      DESTINATION_AQUA: destinationDevnet.aqua,
      DESTINATION_WETH: destinationDevnet.tokens.WETH.address,
      ORIGIN_CHAIN_ID: String(originDevnet.chain_id),
      ORIGIN_SETTLER: predictedSettler,
      ORIGIN_COMPACT: originCompact.compact,
      ORIGIN_TOKEN: originDevnet.tokens.USDC.address,
      ORIGIN_USDC: originDevnet.tokens.USDC.address,
      DESTINATION_FILL_VERIFIER: destinationInfra.inbox,
      DESTINATION_PROOF_OUTBOX: destinationInfra.outbox,
      DESTINATION_REPAYMENT_VERIFIER: destinationInfra.inbox,
      DESTINATION_USDC: destinationDevnet.tokens.USDC.address,
      APP_MANIFEST: toContainerPath(appManifestPath()),
    },
  });
  const app = tryRead<DestinationAppManifest>(appManifestPath())!;
  await assertDeployed(destination.rpcUrl, { app: app.app });
  if (app.app.toLowerCase() !== predictedApp.toLowerCase()) {
    throw new Error(
      `destination app landed at ${app.app}, predicted ${predictedApp} — another transaction ` +
        "used the deployer's nonce concurrently. Stop any other process using this key and rerun.",
    );
  }

  await runForgeScript({
    scriptPath: "script/DeployCrossChainInfra.s.sol",
    contract: "DeployOriginSettler",
    rpcUrl: origin.internalRpcUrl,
    privateKey: DEPLOYER_KEY,
    env: {
      DEVNET_DUMMY,
      ORIGIN_COMPACT: originCompact.compact,
      ORIGIN_AQUA: originDevnet.aqua,
      ORIGIN_TOKEN: originDevnet.tokens.USDC.address,
      DESTINATION_CHAIN_ID: String(destinationDevnet.chain_id),
      DESTINATION_APP: app.app,
      ORIGIN_FILL_VERIFIER: originInfra.inbox,
      ORIGIN_PROOF_OUTBOX: originInfra.outbox,
      ORIGIN_USDC: originDevnet.tokens.USDC.address,
      ORIGIN_SWAP_ROUTER: originDevnet.router,
      DESTINATION_USDC: destinationDevnet.tokens.USDC.address,
      SETTLER_MANIFEST: toContainerPath(settlerManifestPath()),
    },
  });
  const settler = tryRead<OriginSettlerManifest>(settlerManifestPath())!;
  await assertDeployed(origin.rpcUrl, { settler: settler.settler });
  if (settler.settler.toLowerCase() !== predictedSettler.toLowerCase()) {
    throw new Error(
      `origin settler landed at ${settler.settler}, predicted ${predictedSettler} — another ` +
        "transaction used the deployer's nonce concurrently. Stop any other process using this key and rerun.",
    );
  }

  return { settler, app };
}

/** Complete the one-time proof-rail links once both contracts that emit proofs exist. */
export async function configureProofRails(
  origin: ChainTarget,
  destination: ChainTarget,
  originInfra: CrossChainInfraManifest,
  destinationInfra: CrossChainInfraManifest,
  settler: OriginSettlerManifest,
  app: DestinationAppManifest,
): Promise<void> {
  const configure = async (
    target: ChainTarget,
    local: CrossChainInfraManifest,
    remote: CrossChainInfraManifest,
    recorder: string,
  ): Promise<void> => {
    await runForgeScript({
      scriptPath: "script/DeployProofRail.s.sol",
      contract: "ConfigureProofRail",
      rpcUrl: target.internalRpcUrl,
      privateKey: DEPLOYER_KEY,
      env: {
        PROOF_INBOX: local.inbox,
        PROOF_OUTBOX: local.outbox,
        CCIP_REMOTE_OUTBOX: remote.outbox,
        PROOF_RECORDER: recorder,
        CCIP_DESTINATION_INBOX: remote.inbox,
      },
    });
  };

  await configure(origin, originInfra, destinationInfra, settler.settler);
  await configure(destination, destinationInfra, originInfra, app.app);
}
