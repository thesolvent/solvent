// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script } from "forge-std/Script.sol";

import { Aqua } from "@1inch/aqua/src/Aqua.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";

import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";
import { DevToken } from "../src/DevToken.sol";

/// @notice One-shot devnet deploy: the Aqua core + its SwapVM router, the UniswapX reactor + our
///         filler, and a set of mintable test tokens at real-world decimals. Writes an address
///         manifest the backend and frontend read. Devnet only — never a real network.
contract DeployDevnet is Script {
    /// Canonical, chain-agnostic infra already present on the devnet chain (baked into wharfnet's
    /// load-state). Referenced, not deployed.
    address constant PERMIT2 = 0x000000000022D473030F116dDEE9F6B43aC78BA3;
    address constant MULTICALL3 = 0xcA11bde05977b3631167028862bE2a173976CA11;

    /// The router's EIP-712 domain. These MUST match what the backend signs orders against, or no
    /// signature verifies on-chain.
    string constant ROUTER_NAME = "SwapVM";
    string constant ROUTER_VERSION = "1.0.0";

    /// No native wrapping on the devnet, so the router takes no WETH.
    address constant WETH = address(0);

    struct Tok {
        string name;
        string symbol;
        uint8 decimals;
    }

    function run() external {
        Tok[6] memory toks;
        toks[0] = Tok({ name: "USD Coin", symbol: "USDC", decimals: 6 });
        toks[1] = Tok({ name: "Tether USD", symbol: "USDT", decimals: 6 });
        toks[2] = Tok({ name: "Dai Stablecoin", symbol: "DAI", decimals: 18 });
        toks[3] = Tok({ name: "Wrapped Ether", symbol: "WETH", decimals: 18 });
        toks[4] = Tok({ name: "Wrapped BTC", symbol: "WBTC", decimals: 8 });
        toks[5] = Tok({ name: "ChainLink Token", symbol: "LINK", decimals: 18 });

        // The broadcasting key owns the router and the filler (the resolver operator on devnet).
        address owner = msg.sender;

        vm.startBroadcast();

        Aqua aqua = new Aqua();
        AquaSwapVMRouter router = new AquaSwapVMRouter(address(aqua), WETH, owner, ROUTER_NAME, ROUTER_VERSION);
        V2DutchOrderReactor reactor = new V2DutchOrderReactor(IPermit2(PERMIT2), address(0));
        // Devnet: the deployer is also the policy signer, matching SOLVENT_POLICY_SIGNER_KEY.
        UniswapXAquaFiller filler =
            new UniswapXAquaFiller(owner, ISwapVM(address(router)), IReactor(address(reactor)), owner);

        address[6] memory tokenAddrs;
        for (uint256 i = 0; i < toks.length; i++) {
            tokenAddrs[i] = address(new DevToken(toks[i].name, toks[i].symbol, toks[i].decimals));
        }

        vm.stopBroadcast();

        _writeManifest(address(aqua), address(router), address(reactor), address(filler), toks, tokenAddrs);
    }

    /// Emit the address manifest as JSON. `router` is the Aqua app makers ship to; `filler` is the
    /// reactor callback the resolver drives.
    function _writeManifest(
        address aqua,
        address router,
        address reactor,
        address filler,
        Tok[6] memory toks,
        address[6] memory tokenAddrs
    )
        internal
    {
        string memory root = "root";
        vm.serializeUint(root, "chain_id", block.chainid);
        vm.serializeAddress(root, "permit2", PERMIT2);
        vm.serializeAddress(root, "multicall3", MULTICALL3);
        vm.serializeAddress(root, "aqua", aqua);
        vm.serializeAddress(root, "router", router);
        vm.serializeAddress(root, "reactor", reactor);
        vm.serializeAddress(root, "filler", filler);

        string memory tokensObj = "tokens";
        string memory tokensJson;
        for (uint256 i = 0; i < toks.length; i++) {
            string memory one = toks[i].symbol;
            vm.serializeAddress(one, "address", tokenAddrs[i]);
            string memory oneJson = vm.serializeUint(one, "decimals", uint256(toks[i].decimals));
            tokensJson = vm.serializeString(tokensObj, toks[i].symbol, oneJson);
        }

        string memory finalJson = vm.serializeString(root, "tokens", tokensJson);
        string memory outPath = vm.envOr("DEVNET_MANIFEST", string("deployments/solvent-devnet.json"));
        vm.writeJson(finalJson, outPath);
    }
}
