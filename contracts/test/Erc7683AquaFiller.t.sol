// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";

import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { IERC20Errors } from "@openzeppelin/contracts/interfaces/draft-IERC6093.sol";

import { Erc7683AquaFiller } from "../src/Erc7683AquaFiller.sol";
import { SameChainSettler } from "../src/SameChainSettler.sol";
import { GaslessCrossChainOrder, IDestinationSettler } from "../src/interfaces/ERC7683.sol";

/// @notice Hermetic suite proving the zero-inventory claim for ERC-7683: the filler starts with nothing,
///         sources the output from Aqua makers inside SwapVM's pre-transfer-in flash, settles the order,
///         and ends holding only the spread. Everything (Permit2, Aqua/SwapVM, settler, tokens) is
///         deployed locally from source.
contract Erc7683AquaFillerTest is AquaStrategyBuilders, DeployPermit2 {
    bytes internal constant WITNESS_STUB =
        "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
    bytes internal constant WITNESS_TYPE_STRING = "GaslessCrossChainOrder witness)"
        "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,uint256 originChainId,"
        "uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,bytes orderData)"
        "TokenPermissions(address token,uint256 amount)";
    bytes32 internal constant TOKEN_PERMISSIONS_TYPEHASH = keccak256("TokenPermissions(address token,uint256 amount)");

    uint256 internal constant SWAPPER_PK = 0x1010;
    uint256 internal constant IN_AMOUNT = 3100 ether;
    uint256 internal constant OUT_AMOUNT = 1 ether;

    AquaSwapVMRouter internal swapVM;
    ISignatureTransfer internal permit2;
    SameChainSettler internal settler;
    Erc7683AquaFiller internal filler;

    address internal swapper;
    uint256 internal nonce;

    constructor() AquaStrategyBuilders(address(aqua)) { }

    function setUp() public override {
        super.setUp(); // sets maker, tokenA, tokenB
        permit2 = ISignatureTransfer(deployPermit2());
        swapVM = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        settler = new SameChainSettler(permit2);
        filler = new Erc7683AquaFiller(address(this));

        swapper = vm.addr(SWAPPER_PK);
        vm.prank(swapper);
        tokenA.approve(address(permit2), type(uint256).max);
        vm.warp(1_000_000);
    }

    // --------------------------------------------------------------------------------------------
    // Harness
    // --------------------------------------------------------------------------------------------

    function _shipXycMaker(
        address mkr,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 reserveIn,
        uint256 reserveOut,
        uint256 realOut
    )
        internal
        returns (ISwapVM.Order memory order)
    {
        maker = mkr;
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
        shipStrategy(swapVM, order, tokenIn, tokenOut, reserveIn, reserveOut);
        tokenOut.mint(mkr, realOut);
    }

    function _inner(uint256 outAmount) internal view returns (SameChainSettler.SolventOrder memory) {
        return SameChainSettler.SolventOrder({
            inputToken: address(tokenA),
            inputAmount: IN_AMOUNT,
            outputToken: address(tokenB),
            outputAmount: outAmount,
            recipient: swapper,
            exclusiveFiller: address(0),
            exclusivityEnds: 0
        });
    }

    function _openOrder(uint256 outAmount) internal returns (bytes32 id, bytes memory originData) {
        SameChainSettler.SolventOrder memory inner = _inner(outAmount);
        GaslessCrossChainOrder memory order = GaslessCrossChainOrder({
            originSettler: address(settler),
            user: swapper,
            nonce: nonce++,
            originChainId: block.chainid,
            openDeadline: uint32(block.timestamp + 500),
            fillDeadline: uint32(block.timestamp + 1000),
            orderDataType: settler.SOLVENT_ORDER_TYPE_HASH(),
            orderData: abi.encode(inner)
        });

        tokenA.mint(swapper, IN_AMOUNT);
        id = settler.orderIdFor(order);
        originData = order.orderData;
        settler.openFor(order, _sign(order, inner), "");
    }

    function _sign(
        GaslessCrossChainOrder memory order,
        SameChainSettler.SolventOrder memory inner
    )
        internal
        view
        returns (bytes memory)
    {
        bytes32 tokenPermissions =
            keccak256(abi.encode(TOKEN_PERMISSIONS_TYPEHASH, inner.inputToken, inner.inputAmount));
        bytes32 witnessTypeHash = keccak256(abi.encodePacked(WITNESS_STUB, WITNESS_TYPE_STRING));
        bytes32 structHash = keccak256(
            abi.encode(
                witnessTypeHash,
                tokenPermissions,
                address(settler),
                order.nonce,
                uint256(order.openDeadline),
                settler.orderIdFor(order)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", permit2.DOMAIN_SEPARATOR(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(SWAPPER_PK, digest);
        return bytes.concat(r, s, bytes1(v));
    }

    function _source(
        ISwapVM.Order memory order,
        uint256 amountOut,
        uint256 amountInMaximum
    )
        internal
        view
        returns (Erc7683AquaFiller.SourceSwap memory)
    {
        return Erc7683AquaFiller.SourceSwap({
            router: ISwapVM(address(swapVM)),
            order: order,
            tokenIn: address(tokenA),
            tokenOut: address(tokenB),
            amountOut: amountOut,
            amountInMaximum: amountInMaximum
        });
    }

    // --------------------------------------------------------------------------------------------
    // Zero inventory
    // --------------------------------------------------------------------------------------------

    function test_fill_singleLeg_deliversAndKeepsOnlyTheSpread() public {
        ISwapVM.Order memory order = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);

        // The filler holds nothing of either token before the fill.
        assertEq(tokenA.balanceOf(address(filler)), 0, "no input inventory");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");

        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](1);
        sources[0] = _source(order, OUT_AMOUNT, IN_AMOUNT);

        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);

        assertEq(tokenB.balanceOf(swapper), OUT_AMOUNT, "swapper received the output");
        assertEq(tokenB.balanceOf(address(filler)), 0, "filler holds no output after");
        assertGt(tokenA.balanceOf(address(filler)), 0, "filler kept the spread");
        assertEq(tokenA.balanceOf(address(settler)), 0, "escrow released");
    }

    function test_fill_twoLegs_nestsAndSettlesOnce() public {
        address makerA = maker;
        address makerB = vm.addr(0x4444);
        ISwapVM.Order memory orderA = _shipXycMaker(makerA, tokenA, tokenB, 1_200_000 ether, 400 ether, 1 ether);
        ISwapVM.Order memory orderB = _shipXycMaker(makerB, tokenA, tokenB, 1_800_000 ether, 600 ether, 1 ether);

        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);

        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](2);
        sources[0] = _source(orderA, 0.4 ether, IN_AMOUNT);
        sources[1] = _source(orderB, 0.6 ether, IN_AMOUNT);

        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);

        assertEq(tokenB.balanceOf(swapper), OUT_AMOUNT, "swapper received the whole output");
        assertEq(tokenB.balanceOf(makerA), 0.6 ether, "maker A supplied 0.4");
        assertEq(tokenB.balanceOf(makerB), 0.4 ether, "maker B supplied 0.6");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
    }

    function test_fill_fourLegs_atTheBound() public {
        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](4);
        for (uint256 i; i < 4; ++i) {
            ISwapVM.Order memory o =
                _shipXycMaker(vm.addr(0x9000 + i), tokenA, tokenB, 800_000 ether, 300 ether, 1 ether);
            sources[i] = _source(o, 0.25 ether, IN_AMOUNT);
        }

        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);
        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);

        assertEq(tokenB.balanceOf(swapper), OUT_AMOUNT, "four nested legs delivered the full output");
        assertEq(tokenB.balanceOf(address(filler)), 0, "no output inventory");
    }

    function test_fill_revertsAboveMaxLegs() public {
        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](5);
        for (uint256 i; i < 5; ++i) {
            ISwapVM.Order memory o =
                _shipXycMaker(vm.addr(0x9100 + i), tokenA, tokenB, 800_000 ether, 300 ether, 1 ether);
            sources[i] = _source(o, 0.2 ether, IN_AMOUNT);
        }
        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);

        vm.expectRevert(abi.encodeWithSelector(Erc7683AquaFiller.TooManyLegs.selector, 5, 4));
        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);
    }

    function test_fill_revertsWithNoSources() public {
        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);
        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](0);
        vm.expectRevert(Erc7683AquaFiller.NoSources.selector);
        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);
    }

    // Under-sourcing cannot quietly spend the filler's accrued spread: the settler's pull exceeds the
    // approval we granted for what we actually sourced.
    function test_fill_revertsWhenUnderSourced() public {
        ISwapVM.Order memory order = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);
        tokenB.mint(address(filler), 5 ether); // pre-existing spread the guard must protect

        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](1);
        sources[0] = _source(order, OUT_AMOUNT / 2, IN_AMOUNT); // sources only half

        // The settler may pull only what we sourced, so its pull of the order's full amount fails —
        // rather than silently succeeding out of the filler's own 5 ether of accrued spread.
        vm.expectRevert(
            abi.encodeWithSelector(
                IERC20Errors.ERC20InsufficientAllowance.selector, address(settler), OUT_AMOUNT / 2, OUT_AMOUNT
            )
        );
        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);
    }

    function test_fill_revertsForNonOwner() public {
        (bytes32 id, bytes memory originData) = _openOrder(OUT_AMOUNT);
        Erc7683AquaFiller.SourceSwap[] memory sources = new Erc7683AquaFiller.SourceSwap[](1);
        sources[0] = _source(
            _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether), OUT_AMOUNT, IN_AMOUNT
        );

        vm.prank(address(0xBEEF));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBEEF)));
        filler.fill(IDestinationSettler(address(settler)), id, originData, sources);
    }

    // --------------------------------------------------------------------------------------------
    // Callback authentication
    // --------------------------------------------------------------------------------------------

    function test_preTransferInCallback_revertsWhenNoFillInFlight() public {
        vm.expectRevert(
            abi.encodeWithSelector(Erc7683AquaFiller.CallbackUnauthorized.selector, address(this), address(0))
        );
        filler.preTransferInCallback(address(0), address(0), address(0), address(0), 0, 0, bytes32(0), "");
    }

    function test_preTransferOutCallback_alwaysReverts() public {
        vm.expectRevert(Erc7683AquaFiller.CallbackUnsupported.selector);
        filler.preTransferOutCallback(address(0), address(0), address(0), address(0), 0, 0, bytes32(0), "");
    }
}
