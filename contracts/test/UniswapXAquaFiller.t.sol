// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";

import { PermitSignature } from "uniswapx-test/util/PermitSignature.sol";
import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { OrderInfoBuilder } from "uniswapx-test/util/OrderInfoBuilder.sol";
import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { V2DutchOrder, V2DutchOrderLib, CosignerData } from "uniswapx/lib/V2DutchOrderLib.sol";
import { DutchInput, DutchOutput } from "uniswapx/lib/DutchOrderLib.sol";
import { OrderInfo, SignedOrder, ResolvedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ERC20 as SolERC20 } from "solmate/src/tokens/ERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";

/// @notice Composes 1inch's Aqua/SwapVM test base (maker side) with a real UniswapX V2 reactor + Permit2
///         (swapper side) to prove `UniswapXAquaFiller` end to end against unmocked infrastructure.
contract UniswapXAquaFillerTest is AquaStrategyBuilders, PermitSignature, DeployPermit2 {
    using OrderInfoBuilder for OrderInfo;
    using V2DutchOrderLib for V2DutchOrder;

    AquaSwapVMRouter internal swapVM;
    IPermit2 internal permit2;
    V2DutchOrderReactor internal reactor;
    UniswapXAquaFiller internal filler;

    uint256 internal constant SWAPPER_PK = 0x1010;
    uint256 internal constant COSIGNER_PK = 0x2020;
    address internal swapper;

    // tokenA = input the swapper pays (USDC-like); tokenB = output the maker sources (WETH-like).
    uint256 internal nonce;

    event Swept(address indexed token, address indexed to, uint256 amount);

    constructor() AquaStrategyBuilders(address(aqua)) { }

    function setUp() public override {
        super.setUp(); // sets maker, tokenA, tokenB

        swapVM = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        permit2 = IPermit2(deployPermit2());
        reactor = new V2DutchOrderReactor(permit2, address(0));
        filler = new UniswapXAquaFiller(address(this));

        swapper = vm.addr(SWAPPER_PK);
        vm.prank(swapper);
        tokenA.approve(address(permit2), type(uint256).max);
    }

    // --------------------------------------------------------------------------------------------
    // Happy paths
    // --------------------------------------------------------------------------------------------

    function test_fill_singleMaker() public {
        uint256 out = 1 ether;
        uint256 input = 3100 ether;

        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);

        DutchOutput[] memory outputs = _outputs(tokenB, out, swapper);
        SignedOrder memory signed = _signOrder(input, outputs);

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, out, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        uint256 paidToMaker = tokenA.balanceOf(maker); // maker starts with 0 input token
        assertEq(tokenB.balanceOf(swapper), out, "swapper receives output");
        assertEq(tokenA.balanceOf(swapper), 0, "swapper paid full input");
        assertEq(tokenB.balanceOf(address(filler)), 0, "filler holds no output");
        assertEq(tokenA.balanceOf(address(filler)), input - paidToMaker, "filler keeps the spread");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread is positive");
    }

    function test_zeroInventory_afterFill() public {
        uint256 out = 1 ether;
        uint256 input = 3100 ether;

        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signOrder(input, _outputs(tokenB, out, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, out, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        // The only token the filler ends up holding is the spread, in the input token.
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "only the spread remains");
    }

    function test_fill_multiMaker_sourcesRemainder() public {
        uint256 input = 3100 ether;
        address makerA = maker;
        address makerB = vm.addr(0x4444);

        // Neither maker alone holds enough output; the order is sourced from both in one tx.
        (ISwapVM.Order memory orderA,) = _shipXycMaker(makerA, tokenA, tokenB, 1_200_000 ether, 400 ether, 1 ether);
        (ISwapVM.Order memory orderB,) = _shipXycMaker(makerB, tokenA, tokenB, 1_800_000 ether, 600 ether, 1 ether);

        SignedOrder memory signed = _signOrder(input, _outputs(tokenB, 1 ether, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](2);
        sources[0] = _source(orderA, tokenA, tokenB, 0.4 ether, input);
        sources[1] = _source(orderB, tokenA, tokenB, 0.6 ether, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(swapper), 1 ether, "swapper receives full output");
        assertEq(tokenB.balanceOf(makerA), 0.6 ether, "maker A supplied 0.4");
        assertEq(tokenB.balanceOf(makerB), 0.4 ether, "maker B supplied the 0.6 remainder");
        assertEq(tokenB.balanceOf(address(filler)), 0, "filler holds no output");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    function test_fill_multiTokenOutputs() public {
        uint256 input = 5000 ether;
        address makerB = maker;
        address makerC = vm.addr(0x5555);
        TokenMock tokenC = new TokenMock("Token C", "TKC");

        (ISwapVM.Order memory orderB,) = _shipXycMaker(makerB, tokenA, tokenB, 3_000_000 ether, 1000 ether, 5 ether);
        (ISwapVM.Order memory orderC,) = _shipXycMaker(makerC, tokenA, tokenC, 1_000_000 ether, 2000 ether, 5 ether);

        DutchOutput[] memory outputs = new DutchOutput[](2);
        outputs[0] =
            DutchOutput({ token: address(tokenB), startAmount: 1 ether, endAmount: 1 ether, recipient: swapper });
        outputs[1] =
            DutchOutput({ token: address(tokenC), startAmount: 2 ether, endAmount: 2 ether, recipient: swapper });
        SignedOrder memory signed = _signOrder(input, outputs);

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](2);
        sources[0] = _source(orderB, tokenA, tokenB, 1 ether, input);
        sources[1] = _source(orderC, tokenA, tokenC, 2 ether, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(swapper), 1 ether, "swapper receives output B");
        assertEq(tokenC.balanceOf(swapper), 2 ether, "swapper receives output C");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no B inventory");
        assertEq(tokenC.balanceOf(address(filler)), 0, "no C inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    function test_fillBatch_multipleOrders() public {
        uint256 input1 = 3100 ether;
        uint256 input2 = 1600 ether;

        // One maker sources the combined output for both orders.
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 4_500_000 ether, 1500 ether, 5 ether);

        SignedOrder[] memory signed = new SignedOrder[](2);
        signed[0] = _signOrder(input1, _outputs(tokenB, 1 ether, swapper));
        signed[1] = _signOrder(input2, _outputs(tokenB, 0.5 ether, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1.5 ether, input1 + input2);

        _mintSwapper(input1 + input2);
        filler.fillBatch(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(swapper), 1.5 ether, "swapper receives both orders' output");
        assertEq(tokenB.balanceOf(address(filler)), 0, "filler holds no output");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    // --------------------------------------------------------------------------------------------
    // Boundaries — the guards are inclusive/exclusive exactly where intended
    // --------------------------------------------------------------------------------------------

    function test_fill_atExactMaxInput() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        uint256 amountIn = _quoteAmountIn(order, address(tokenA), address(tokenB), 1 ether);

        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, amountIn); // cap == exact cost (inclusive)

        _mintSwapper(3100 ether);
        filler.fill(IReactor(address(reactor)), signed, sources);
        assertEq(tokenB.balanceOf(swapper), 1 ether, "fills when input hits the cap exactly");
    }

    function test_revert_legExceedsAmountInMaximum() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        uint256 amountIn = _quoteAmountIn(order, address(tokenA), address(tokenB), 1 ether);

        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, amountIn - 1); // 1 wei below cost

        _mintSwapper(3100 ether);
        vm.expectRevert(); // SwapVM taker-traits threshold: amountIn > amountInMaximum
        filler.fill(IReactor(address(reactor)), signed, sources);
    }

    function test_fill_breakEven() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        uint256 amountIn = _quoteAmountIn(order, address(tokenA), address(tokenB), 1 ether);

        uint256 seed = 1000 ether;
        tokenA.mint(address(filler), seed); // pre-existing spread inventory

        SignedOrder memory signed = _signOrder(amountIn, _outputs(tokenB, 1 ether, swapper)); // input == cost
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, amountIn);

        _mintSwapper(amountIn);
        filler.fill(IReactor(address(reactor)), signed, sources); // net zero -> guard passes (>=)
        assertEq(tokenA.balanceOf(address(filler)), seed, "break-even leaves inventory exactly intact");
    }

    // --------------------------------------------------------------------------------------------
    // Profitability guard — protects the treasury against a bad plan
    // --------------------------------------------------------------------------------------------

    function test_fill_preservesExistingInventory() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        uint256 seed = 1000 ether;
        tokenA.mint(address(filler), seed);

        uint256 input = 3100 ether;
        SignedOrder memory signed = _signOrder(input, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        uint256 paid = tokenA.balanceOf(maker);
        assertEq(tokenA.balanceOf(address(filler)), seed + input - paid, "seed preserved, spread added on top");
        assertGt(tokenA.balanceOf(address(filler)), seed, "spread accrues above the seed");
    }

    function test_revert_profitabilityGuard() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        uint256 amountIn = _quoteAmountIn(order, address(tokenA), address(tokenB), 1 ether);

        tokenA.mint(address(filler), 1000 ether); // treasury the bad plan would eat into
        uint256 input = amountIn - 100; // swapper pays less than the maker charges

        SignedOrder memory signed = _signOrder(input, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, amountIn); // cap lets the router charge full cost

        _mintSwapper(input);
        // The input token snapshots at the seed and ends 100 below it (amountIn cancels out).
        vm.expectRevert(
            abi.encodeWithSelector(
                UniswapXAquaFiller.ProfitabilityGuard.selector,
                address(tokenA),
                uint256(1000 ether),
                uint256(1000 ether) - 100
            )
        );
        filler.fill(IReactor(address(reactor)), signed, sources);
    }

    // --------------------------------------------------------------------------------------------
    // Output sourcing + settlement
    // --------------------------------------------------------------------------------------------

    function test_revert_outputNotSourced() public {
        _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));

        // A plan that sources nothing for the required output token.
        UniswapXAquaFiller.SourceSwap[] memory none = new UniswapXAquaFiller.SourceSwap[](0);

        _mintSwapper(3100 ether);
        vm.expectRevert(abi.encodeWithSelector(UniswapXAquaFiller.OutputNotSourced.selector, address(tokenB)));
        filler.fill(IReactor(address(reactor)), signed, none);
    }

    function test_revert_underSourcedPlan() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));

        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 0.5 ether, 3100 ether); // sources only half the output

        _mintSwapper(3100 ether);
        vm.expectRevert(); // reactor _fill pulls 1 ether from a filler holding 0.5
        filler.fill(IReactor(address(reactor)), signed, sources);
    }

    function test_revert_makerRealBalanceInsufficient_rollsBackAtomically() public {
        address makerA = maker;
        address makerB = vm.addr(0x4444);

        (ISwapVM.Order memory orderA,) = _shipXycMaker(makerA, tokenA, tokenB, 1_200_000 ether, 400 ether, 1 ether);
        // Maker B ships a position but holds NO real output -> its Aqua pull will revert.
        (ISwapVM.Order memory orderB,) = _shipXycMaker(makerB, tokenA, tokenB, 1_800_000 ether, 600 ether, 0);

        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](2);
        sources[0] = _source(orderA, tokenA, tokenB, 0.4 ether, 3100 ether);
        sources[1] = _source(orderB, tokenA, tokenB, 0.6 ether, 3100 ether);

        _mintSwapper(3100 ether);
        uint256 makerAbefore = tokenB.balanceOf(makerA);

        vm.expectRevert(); // Aqua pull from maker B (zero real balance)
        filler.fill(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(makerA), makerAbefore, "maker A's leg rolled back");
        assertEq(tokenA.balanceOf(swapper), 3100 ether, "swapper funds untouched");
        assertEq(tokenB.balanceOf(swapper), 0, "no partial delivery");
    }

    function test_fill_resetsRouterAllowance() public {
        (ISwapVM.Order memory order,) = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        SignedOrder memory signed = _signOrder(3100 ether, _outputs(tokenB, 1 ether, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](1);
        sources[0] = _source(order, tokenA, tokenB, 1 ether, 3100 ether);

        _mintSwapper(3100 ether);
        filler.fill(IReactor(address(reactor)), signed, sources);
        assertEq(tokenA.allowance(address(filler), address(swapVM)), 0, "router allowance cleared after the fill");
    }

    // --------------------------------------------------------------------------------------------
    // Access control + admin
    // --------------------------------------------------------------------------------------------

    function test_revert_reactorCallback_unauthorized() public {
        ResolvedOrder[] memory empty = new ResolvedOrder[](0);
        // Direct call with no fill in flight: _reactorInFlight == 0, so any caller is rejected.
        vm.expectRevert(
            abi.encodeWithSelector(UniswapXAquaFiller.CallbackUnauthorized.selector, address(this), address(0))
        );
        filler.reactorCallback(empty, "");
    }

    function test_revert_onlyOwner() public {
        address attacker = address(0xBAD);
        UniswapXAquaFiller.SourceSwap[] memory none = new UniswapXAquaFiller.SourceSwap[](0);
        SignedOrder memory dummy;
        SignedOrder[] memory dummyBatch = new SignedOrder[](0);

        vm.startPrank(attacker);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, attacker));
        filler.fill(IReactor(address(reactor)), dummy, none);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, attacker));
        filler.fillBatch(IReactor(address(reactor)), dummyBatch, none);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, attacker));
        filler.sweep(address(tokenA), attacker);
        vm.stopPrank();
    }

    function test_ownership_twoStepTransfer() public {
        address newOwner = address(0xA11CE);
        filler.transferOwnership(newOwner);
        assertEq(filler.pendingOwner(), newOwner, "pending owner set");
        assertEq(filler.owner(), address(this), "owner unchanged until accepted");

        vm.prank(address(0xBAD));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBAD)));
        filler.acceptOwnership();

        vm.prank(newOwner);
        filler.acceptOwnership();
        assertEq(filler.owner(), newOwner, "ownership transferred on accept");
    }

    function test_sweep_collectsSpreadAndEmits() public {
        address treasury = address(0x7EA);
        tokenA.mint(address(filler), 500 ether);

        vm.expectEmit(true, true, false, true, address(filler));
        emit Swept(address(tokenA), treasury, 500 ether);
        filler.sweep(address(tokenA), treasury);

        assertEq(tokenA.balanceOf(treasury), 500 ether, "spread swept out");
        assertEq(tokenA.balanceOf(address(filler)), 0, "filler emptied");

        filler.sweep(address(tokenB), treasury); // zero balance is a no-op, not a revert
        assertEq(tokenB.balanceOf(treasury), 0);
    }

    function test_revert_sweep_zeroRecipient() public {
        tokenA.mint(address(filler), 1 ether);
        vm.expectRevert(UniswapXAquaFiller.ZeroAddress.selector);
        filler.sweep(address(tokenA), address(0));
    }

    // --------------------------------------------------------------------------------------------
    // Fuzz — arbitrary splits of one order's output across two makers settle correctly
    // --------------------------------------------------------------------------------------------

    function testFuzz_sourceSplit(uint256 splitBps) public {
        splitBps = bound(splitBps, 100, 9900); // 1%..99% to maker A
        uint256 out = 1 ether;
        uint256 outA = (out * splitBps) / 10_000;
        uint256 outB = out - outA;
        uint256 input = 5000 ether;

        address makerA = maker;
        address makerB = vm.addr(0x4444);
        (ISwapVM.Order memory orderA,) = _shipXycMaker(makerA, tokenA, tokenB, 3_000_000 ether, 1000 ether, 100 ether);
        (ISwapVM.Order memory orderB,) = _shipXycMaker(makerB, tokenA, tokenB, 3_000_000 ether, 1000 ether, 100 ether);

        SignedOrder memory signed = _signOrder(input, _outputs(tokenB, out, swapper));
        UniswapXAquaFiller.SourceSwap[] memory sources = new UniswapXAquaFiller.SourceSwap[](2);
        sources[0] = _source(orderA, tokenA, tokenB, outA, input);
        sources[1] = _source(orderB, tokenA, tokenB, outB, input);

        _mintSwapper(input);
        filler.fill(IReactor(address(reactor)), signed, sources);

        assertEq(tokenB.balanceOf(swapper), out, "swapper receives the full output for any split");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    // --------------------------------------------------------------------------------------------
    // Harness helpers
    // --------------------------------------------------------------------------------------------

    /// @dev Exact input the SwapVM would charge for an exact-out swap — 22 zero bytes = exact-out taker
    ///      traits with no threshold/hooks. Uses the Simulator's `asView()` staticcall wrapper.
    function _quoteAmountIn(
        ISwapVM.Order memory order,
        address tokenIn,
        address tokenOut,
        uint256 amountOut
    )
        internal
        returns (uint256 amountIn)
    {
        bytes memory takerData = new bytes(22);
        (amountIn,,) = swapVM.asView().quote(order, tokenIn, tokenOut, amountOut, takerData);
    }

    /// @dev Ship an XYC Aqua strategy for `mkr` with the given virtual reserves, and mint it `realOut`
    ///      of the output token so Aqua's `pull` can settle.
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

    function _source(
        ISwapVM.Order memory order,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 amountOut,
        uint256 amountInMaximum
    )
        internal
        view
        returns (UniswapXAquaFiller.SourceSwap memory)
    {
        return UniswapXAquaFiller.SourceSwap({
            router: ISwapVM(address(swapVM)),
            order: order,
            tokenIn: address(tokenIn),
            tokenOut: address(tokenOut),
            amountOut: amountOut,
            amountInMaximum: amountInMaximum
        });
    }

    function _outputs(
        TokenMock token,
        uint256 amount,
        address recipient
    )
        internal
        pure
        returns (DutchOutput[] memory outputs)
    {
        outputs = new DutchOutput[](1);
        outputs[0] =
            DutchOutput({ token: address(token), startAmount: amount, endAmount: amount, recipient: recipient });
    }

    function _signOrder(uint256 inputAmount, DutchOutput[] memory outputs)
        internal
        returns (SignedOrder memory signed)
    {
        OrderInfo memory info = OrderInfoBuilder.init(address(reactor)).withSwapper(swapper)
            .withDeadline(block.timestamp + 1000).withNonce(nonce++);

        CosignerData memory cosignerData = CosignerData({
            decayStartTime: block.timestamp,
            decayEndTime: info.deadline,
            exclusiveFiller: address(0),
            exclusivityOverrideBps: 0,
            inputAmount: 0,
            outputAmounts: new uint256[](outputs.length)
        });

        V2DutchOrder memory order = V2DutchOrder({
            info: info,
            cosigner: vm.addr(COSIGNER_PK),
            baseInput: DutchInput({
                token: SolERC20(address(tokenA)), startAmount: inputAmount, endAmount: inputAmount
            }),
            baseOutputs: outputs,
            cosignerData: cosignerData,
            cosignature: ""
        });

        bytes32 orderHash = order.hash();
        order.cosignature = _cosign(orderHash, cosignerData);
        signed = SignedOrder({ order: abi.encode(order), sig: signOrder(SWAPPER_PK, address(permit2), order) });
    }

    function _cosign(bytes32 orderHash, CosignerData memory cosignerData) internal pure returns (bytes memory) {
        bytes32 msgHash = keccak256(abi.encodePacked(orderHash, abi.encode(cosignerData)));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(COSIGNER_PK, msgHash);
        return bytes.concat(r, s, bytes1(v));
    }

    function _mintSwapper(uint256 inputAmount) internal {
        tokenA.mint(swapper, inputAmount);
    }
}
