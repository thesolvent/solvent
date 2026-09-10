// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { SignedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";
import { UniswapXAquaFillerHarness } from "./UniswapXAquaFillerHarness.sol";

/// @notice Opt-in realism suite: runs the same fill against the REAL mainnet-deployed UniswapX V2 reactor
///         and canonical Permit2, with Aqua/SwapVM + mock tokens deployed on the fork (1inch has not
///         published Aqua/SwapVM mainnet addresses yet — migrate to fork-by-address once they do).
/// @dev Skips cleanly when `MAINNET_RPC_URL` is unset, so the default `forge test` stays hermetic offline.
contract UniswapXAquaFillerForkTest is UniswapXAquaFillerHarness {
    // Real mainnet deployments.
    address internal constant V2_DUTCH_ORDER_REACTOR = 0x00000011F84B9aa48e5f8aA8B9897600006289Be;
    address internal constant PERMIT2 = 0x000000000022D473030F116dDEE9F6B43aC78BA3;

    function setUp() public override {
        string memory rpc = vm.envOr("MAINNET_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            return; // not forked; the test self-skips
        }

        // Keep the Aqua deployed at construction alive across the fork switch, then adopt the real reactor.
        vm.makePersistent(address(aqua));
        vm.createSelectFork(rpc);

        reactor = V2DutchOrderReactor(payable(V2_DUTCH_ORDER_REACTOR));
        permit2 = IPermit2(PERMIT2);
        _deploy();

        // Some mainnet addresses carry EIP-7702 delegated code, which would send Permit2 down the
        // EIP-1271 path; clear it so the swapper is a plain EOA and its ECDSA signature is checked.
        vm.etch(swapper, "");
    }

    function testFork_fill_realV2Reactor() public {
        if (address(reactor) == address(0)) {
            vm.skip(true); // MAINNET_RPC_URL not set
            return;
        }

        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(_userFillContext(signed), order, tokenA, tokenB, 1 ether, 3100 ether);

        _mintSwapper(3100 ether);
        filler.fill(signed, sources);

        assertEq(tokenB.balanceOf(swapper), 1 ether, "swapper paid by the real mainnet reactor");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }
}
