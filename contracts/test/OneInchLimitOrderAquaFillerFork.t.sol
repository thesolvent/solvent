// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { IOrderMixin } from "@1inch/limit-order-protocol-contract/contracts/interfaces/IOrderMixin.sol";
import { LimitOrderProtocol } from "@1inch/limit-order-protocol-contract/contracts/LimitOrderProtocol.sol";

import { OneInchLimitOrderAquaFiller } from "../src/OneInchLimitOrderAquaFiller.sol";
import { OneInchLimitOrderAquaFillerHarness } from "./OneInchLimitOrderAquaFillerHarness.sol";

/// @notice Opt-in realism suite: runs the same fill against the REAL mainnet-deployed 1inch
///         Aggregation Router V6 (which serves the Limit Order Protocol v4 interface), with
///         Aqua/SwapVM + mock tokens deployed on the fork.
/// @dev Skips cleanly when `MAINNET_RPC_URL` is unset, so the default `forge test` stays hermetic
///      offline.
contract OneInchLimitOrderAquaFillerForkTest is OneInchLimitOrderAquaFillerHarness {
    // The real mainnet deployment. Same address the production `OneInchNormalizer`/router lookup
    // uses (`codec::router_address`), confirmed there against the contract's own `eip712Domain()`.
    address internal constant AGGREGATION_ROUTER_V6 = 0x111111125421cA6dc452d289314280a0f8842A65;

    function setUp() public override {
        string memory rpc = vm.envOr("MAINNET_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            return; // not forked; the test self-skips
        }

        // Keep the Aqua deployed at construction alive across the fork switch, then adopt the real
        // router.
        vm.makePersistent(address(aqua));
        vm.createSelectFork(rpc);

        super.setUp(); // sets maker, tokenA, tokenB
        swapVM = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        protocol = LimitOrderProtocol(payable(AGGREGATION_ROUTER_V6));
        filler = new OneInchLimitOrderAquaFiller(address(this), IOrderMixin(address(protocol)));
        signer = vm.addr(SIGNER_PK);
    }

    /// @dev `_signPlainOrder` calls `protocol.hashOrder`, a real, unmodified view call into the
    /// mainnet-deployed contract's own EIP-712 domain — proof our order encoding matches what the
    /// live bytecode actually expects, not just what the vendored source we compiled against says
    /// it should.
    function testFork_fill_realAggregationRouterV6() public {
        if (address(protocol) == address(0)) {
            vm.skip(true); // MAINNET_RPC_URL not set
            return;
        }

        uint256 out = 1 ether;
        uint256 makingAmount = 3100 ether;

        ISwapVM.Order memory makerOrder = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        (IOrderMixin.Order memory order, bytes32 r, bytes32 vs) = _signPlainOrder(tokenA, tokenB, makingAmount, out);

        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](1);
        sources[0] = _source(makerOrder, tokenA, tokenB, out, makingAmount);

        filler.fill(order, r, vs, makingAmount, MAKER_AMOUNT_FLAG, "", sources);

        assertEq(tokenB.balanceOf(signer), out, "signer paid by the real mainnet router");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }
}
