// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script } from "forge-std/Script.sol";

import { Aqua } from "@1inch/aqua/src/Aqua.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";

import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { Erc7683AquaFiller } from "../src/Erc7683AquaFiller.sol";
import { Solvent7683Resolver } from "../src/Solvent7683Resolver.sol";
import { SolventSameChainSettler } from "../src/SolventSameChainSettler.sol";
import { SolventTakerCredential } from "../src/SolventTakerCredential.sol";
import { IErc7683AquaFiller, ISolventSameChainSettler } from "../src/interfaces/ISolvent7683.sol";
import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";
import { DevToken } from "../src/DevToken.sol";

/// @notice One-shot devnet deploy for Aqua, the supported protocol fillers, their settlement
///         dependencies, and mintable test tokens. Writes the manifest consumed offchain.
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

        address deployer = msg.sender;
        uint256 fillerOwnerKey = vm.envUint("FILLER_OWNER_KEY");
        address fillerOwner = vm.addr(fillerOwnerKey);
        address policySigner = vm.addr(vm.envUint("POLICY_SIGNER_KEY"));

        vm.startBroadcast();

        Aqua aqua = new Aqua();
        AquaSwapVMRouter router = new AquaSwapVMRouter(address(aqua), WETH, deployer, ROUTER_NAME, ROUTER_VERSION);
        V2DutchOrderReactor reactor = new V2DutchOrderReactor(IPermit2(PERMIT2), address(0));
        SolventTakerCredential credential = new SolventTakerCredential(deployer);
        SolventSameChainSettler settler = new SolventSameChainSettler(ISignatureTransfer(PERMIT2), credential);
        UniswapXAquaFiller filler =
            new UniswapXAquaFiller(fillerOwner, ISwapVM(address(router)), reactor, credential, policySigner);
        Erc7683AquaFiller erc7683Filler =
            new Erc7683AquaFiller(fillerOwner, ISwapVM(address(router)), settler, credential, policySigner);

        credential.setTaker(address(filler), true);
        credential.setTaker(address(erc7683Filler), true);
        credential.freeze();
        Solvent7683Resolver erc7683Resolver = new Solvent7683Resolver(
            ISolventSameChainSettler(address(settler)), IErc7683AquaFiller(address(erc7683Filler))
        );

        address[6] memory tokenAddrs;
        for (uint256 i = 0; i < toks.length; i++) {
            tokenAddrs[i] = address(new DevToken(toks[i].name, toks[i].symbol, toks[i].decimals));
        }

        vm.stopBroadcast();

        vm.startBroadcast(fillerOwnerKey);
        for (uint256 i = 0; i < tokenAddrs.length; i++) {
            filler.setTokenAllowed(tokenAddrs[i], true);
            erc7683Filler.setTokenAllowed(tokenAddrs[i], true);
        }
        vm.stopBroadcast();

        _writeManifest(
            address(aqua),
            address(router),
            address(reactor),
            address(filler),
            address(settler),
            address(erc7683Filler),
            address(erc7683Resolver),
            address(credential),
            toks,
            tokenAddrs
        );
    }

    /// Emit the address manifest as JSON for backend and frontend startup.
    function _writeManifest(
        address aqua,
        address router,
        address reactor,
        address filler,
        address erc7683Settler,
        address erc7683Filler,
        address erc7683Resolver,
        address takerCredential,
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
        vm.serializeAddress(root, "erc7683_settler", erc7683Settler);
        vm.serializeAddress(root, "erc7683_filler", erc7683Filler);
        vm.serializeAddress(root, "erc7683_resolver", erc7683Resolver);
        vm.serializeAddress(root, "taker_credential", takerCredential);

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
