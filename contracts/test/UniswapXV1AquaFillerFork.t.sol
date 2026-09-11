// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";

import { PermitSignature } from "uniswapx-test/util/PermitSignature.sol";
import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { OrderInfoBuilder } from "uniswapx-test/util/OrderInfoBuilder.sol";
import { ExclusiveDutchOrderReactor } from "uniswapx/reactors/ExclusiveDutchOrderReactor.sol";
import { ExclusiveDutchOrder, ExclusiveDutchOrderLib } from "uniswapx/lib/ExclusiveDutchOrderLib.sol";
import { DutchInput, DutchOutput } from "uniswapx/lib/DutchOrderLib.sol";
import { OrderInfo, SignedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ERC20 as SolERC20 } from "solmate/src/tokens/ERC20.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";

/// @notice Opt-in realism suite for the Orders API's `Limit` order type: runs the same, UNCHANGED
///         `UniswapXAquaFiller` against the REAL mainnet-deployed `ExclusiveDutchOrderReactor`
///         (`0x6000da47483062A0D734Ba3dc7576Ce6A0B645C4`) — the reactor `UniswapXV1Normalizer`
///         decodes orders for — proving the contract fills a real V1-reactor order for real, not
///         just that the Rust side can decode one. `fill(reactor, order, sources)` already takes
///         the reactor as a parameter (see `UniswapXAquaFiller.sol`), so no contract change was
///         needed to support this reactor generation — this test is the proof.
/// @dev Skips cleanly when `MAINNET_RPC_URL` is unset, so the default `forge test` stays hermetic.
///      Mirrors `UniswapXAquaFillerFork.t.sol`'s pattern; not sharing its harness since that one is
///      wired to the V2 order/reactor types specifically.
contract UniswapXV1AquaFillerForkTest is AquaStrategyBuilders, PermitSignature, DeployPermit2 {
    using OrderInfoBuilder for OrderInfo;
    using ExclusiveDutchOrderLib for ExclusiveDutchOrder;

    // The real mainnet deployment `UniswapXV1Normalizer`/`OrdersApiClient` target for `Limit` orders.
    address internal constant V1_REACTOR = 0x6000da47483062A0D734Ba3dc7576Ce6A0B645C4;

    AquaSwapVMRouter internal swapVM;
    IPermit2 internal permit2;
    ExclusiveDutchOrderReactor internal reactor;
    UniswapXAquaFiller internal filler;

    uint256 internal constant SWAPPER_PK = 0x1010;
    address internal swapper;

    constructor() AquaStrategyBuilders(address(aqua)) { }

    function setUp() public override {
        string memory rpc = vm.envOr("MAINNET_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            return; // not forked; the test self-skips
        }

        vm.makePersistent(address(aqua));
        vm.createSelectFork(rpc);

        super.setUp(); // sets maker, tokenA, tokenB
        swapVM = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        filler = new UniswapXAquaFiller(address(this));
        reactor = ExclusiveDutchOrderReactor(payable(V1_REACTOR));
        permit2 = IPermit2(0x000000000022D473030F116dDEE9F6B43aC78BA3);

        swapper = vm.addr(SWAPPER_PK);
        // Some mainnet addresses carry EIP-7702 delegated code, which would send Permit2 down the
        // EIP-1271 path; clear it so the swapper is a plain EOA and its ECDSA signature is checked.
        vm.etch(swapper, "");
        vm.prank(swapper);
        tokenA.approve(address(permit2), type(uint256).max);
    }

    function testFork_fill_realExclusiveDutchOrderReactor() public {
        if (address(reactor) == address(0)) {
            vm.skip(true); // MAINNET_RPC_URL not set
            return;
        }

        uint256 inputAmount = 3100 ether;
        uint256 outputAmount = 1 ether;

        (ISwapVM.Order memory makerOrder,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);

        DutchOutput[] memory outputs = new DutchOutput[](1);
        outputs[0] = DutchOutput({
            token: address(tokenB), startAmount: outputAmount, endAmount: outputAmount, recipient: swapper
        });

        OrderInfo memory info = OrderInfoBuilder.init(address(reactor)).withSwapper(swapper)
            .withDeadline(block.timestamp + 1000).withNonce(1);

        ExclusiveDutchOrder memory order = ExclusiveDutchOrder({
            info: info,
            decayStartTime: block.timestamp,
            decayEndTime: info.deadline,
            exclusiveFiller: address(0),
            exclusivityOverrideBps: 0,
            input: DutchInput({ token: SolERC20(address(tokenA)), startAmount: inputAmount, endAmount: inputAmount }),
            outputs: outputs
        });

        tokenA.mint(swapper, inputAmount);
        SignedOrder memory signed =
            SignedOrder({ order: abi.encode(order), sig: signOrder(SWAPPER_PK, address(permit2), order) });

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = UniswapXAquaFiller.SourceSwap({
            router: ISwapVM(address(swapVM)),
            order: makerOrder,
            tokenIn: address(tokenA),
            tokenOut: address(tokenB),
            amountOut: outputAmount,
            amountInMaximum: inputAmount
        });

        filler.fill(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(swapper), outputAmount, "swapper paid by the real V1 reactor");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    /// @dev Ship an XYC Aqua strategy for `mkr` with the given virtual reserves, and mint it
    ///      `realOut` of the output token so Aqua's `pull` can settle. Copied from
    ///      `UniswapXAquaFillerHarness` rather than inherited — that harness's `reactor`/`filler`
    ///      fields are typed for V2, and this suite needs its own V1-typed ones.
    function _shipXycMaker(
        address mkr,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 reserveIn,
        uint256 reserveOut,
        uint256 realOut
    )
        internal
        returns (ISwapVM.Order memory order, bytes32 strategyHash)
    {
        maker = mkr; // createStrategy/shipStrategy read the inherited `maker`
        order = createStrategy(
            MakerSetup({
                balanceA: 0,
                balanceB: 0,
                priceMin: 0,
                priceMax: 0,
                protocolFeeBps: 0,
                feeInBps: 0,
                protocolFeeRecipient: address(0),
                swapType: SwapType.XYC
            })
        );
        strategyHash = shipStrategy(swapVM, order, tokenIn, tokenOut, reserveIn, reserveOut);
        tokenOut.mint(mkr, realOut);
    }
}
