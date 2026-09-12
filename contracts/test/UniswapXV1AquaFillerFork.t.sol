// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { ExclusiveDutchOrder, ExclusiveDutchOrderLib } from "uniswapx/lib/ExclusiveDutchOrderLib.sol";
import { DutchInput, DutchOutput } from "uniswapx/lib/DutchOrderLib.sol";
import { OrderInfo, SignedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ERC20 as SolERC20 } from "solmate/src/tokens/ERC20.sol";

import { OrderInfoBuilder } from "uniswapx-test/util/OrderInfoBuilder.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";
import { UniswapXAquaFillerHarness } from "./UniswapXAquaFillerHarness.sol";

/// @notice Opt-in realism suite for the Orders API's `Limit` order type: runs the same, UNCHANGED
///         policy-authorized `UniswapXAquaFiller` against the REAL mainnet-deployed
///         `ExclusiveDutchOrderReactor` (`0x6000da47483062A0D734Ba3dc7576Ce6A0B645C4`) — the
///         reactor `UniswapXV1Normalizer` decodes orders for — proving the contract fills a real
///         V1-reactor order for real, not just that the Rust side can decode one. The filler binds
///         its reactor at construction (one deployed instance per reactor generation), so this
///         suite deploys its own instance pointed at the V1 reactor rather than sharing the V2
///         suite's.
/// @dev Skips cleanly when `MAINNET_RPC_URL` is unset, so the default `forge test` stays hermetic.
///      Reuses `UniswapXAquaFillerHarness` for the maker/Aqua/filler machinery (identical to V2's);
///      `reactor`'s static type is the harness's `V2DutchOrderReactor`, but `_deploy()` only ever
///      reads `address(reactor)` — never a V2-specific method — so holding the real V1 reactor's
///      address there is safe. Order building/signing is V1's own: `ExclusiveDutchOrder` has no
///      cosigner, unlike V2.
contract UniswapXV1AquaFillerForkTest is UniswapXAquaFillerHarness {
    using ExclusiveDutchOrderLib for ExclusiveDutchOrder;
    using OrderInfoBuilder for OrderInfo;

    // The real mainnet deployment `UniswapXV1Normalizer`/`OrdersApiClient` target for `Limit` orders.
    address internal constant V1_REACTOR = 0x6000da47483062A0D734Ba3dc7576Ce6A0B645C4;
    address internal constant PERMIT2 = 0x000000000022D473030F116dDEE9F6B43aC78BA3;

    function setUp() public override {
        string memory rpc = vm.envOr("MAINNET_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            return; // not forked; the test self-skips
        }

        vm.makePersistent(address(aqua));
        vm.createSelectFork(rpc);

        reactor = V2DutchOrderReactor(payable(V1_REACTOR)); // address only; never called as V2-typed
        permit2 = IPermit2(PERMIT2);
        _deploy();

        // Some mainnet addresses carry EIP-7702 delegated code, which would send Permit2 down the
        // EIP-1271 path; clear it so the swapper is a plain EOA and its ECDSA signature is checked.
        vm.etch(swapper, "");
    }

    function testFork_fill_realExclusiveDutchOrderReactor() public {
        if (address(reactor) == address(0)) {
            vm.skip(true); // MAINNET_RPC_URL not set
            return;
        }

        uint256 inputAmount = 3100 ether;
        uint256 outputAmount = 1 ether;

        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signV1Order(inputAmount, _outputs(tokenB, outputAmount, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(_userFillContext(signed), order, tokenA, tokenB, outputAmount, inputAmount);

        _mintSwapper(inputAmount);
        filler.fill(signed, sources);

        assertEq(tokenB.balanceOf(swapper), outputAmount, "swapper paid by the real V1 reactor");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    /// @dev V1's own order type — no cosigner, unlike the harness's V2-specific `_signOrder`.
    function _signV1Order(uint256 inputAmount, DutchOutput[] memory outputs) internal returns (SignedOrder memory) {
        OrderInfo memory info = OrderInfoBuilder.init(address(reactor)).withSwapper(swapper)
            .withDeadline(block.timestamp + 1000).withNonce(nonce++);

        ExclusiveDutchOrder memory order = ExclusiveDutchOrder({
            info: info,
            decayStartTime: block.timestamp,
            decayEndTime: info.deadline,
            exclusiveFiller: address(0),
            exclusivityOverrideBps: 0,
            input: DutchInput({ token: SolERC20(address(tokenA)), startAmount: inputAmount, endAmount: inputAmount }),
            outputs: outputs
        });

        return SignedOrder({ order: abi.encode(order), sig: signOrder(SWAPPER_PK, address(permit2), order) });
    }
}
