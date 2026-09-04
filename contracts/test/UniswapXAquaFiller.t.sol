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
import { OrderInfo, SignedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ERC20 as SolERC20 } from "solmate/src/tokens/ERC20.sol";

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
    // Harness helpers
    // --------------------------------------------------------------------------------------------

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
