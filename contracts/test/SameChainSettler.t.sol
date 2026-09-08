// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";

import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { SameChainSettler } from "../src/SameChainSettler.sol";
import { GaslessCrossChainOrder, OnchainCrossChainOrder, ResolvedCrossChainOrder } from "../src/interfaces/ERC7683.sol";

/// @notice Hermetic suite for the ERC-7683 same-chain settler: Permit2 is deployed locally, so the whole
///         open → fill → settle lifecycle runs offline against unmocked infrastructure.
/// @dev The EIP-712 strings are duplicated here on purpose. A test that borrowed the contract's own
///      constants could not detect the contract changing them; hard-coding is what makes them a pin.
contract SameChainSettlerTest is Test, DeployPermit2 {
    bytes internal constant WITNESS_STUB =
        "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
    bytes internal constant WITNESS_TYPE_STRING = "GaslessCrossChainOrder witness)"
        "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,uint256 originChainId,"
        "uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,bytes orderData)"
        "TokenPermissions(address token,uint256 amount)";
    bytes32 internal constant TOKEN_PERMISSIONS_TYPEHASH = keccak256("TokenPermissions(address token,uint256 amount)");
    bytes32 internal constant SOLVENT_ORDER_TYPE_HASH = keccak256(
        "SolventOrder(address inputToken,uint256 inputAmount,address outputToken,uint256 outputAmount,"
        "address recipient,address exclusiveFiller,uint32 exclusivityEnds)"
    );

    uint256 internal constant SWAPPER_PK = 0x1010;

    ISignatureTransfer internal permit2;
    SameChainSettler internal settler;
    TokenMock internal tokenIn;
    TokenMock internal tokenOut;

    address internal swapper;
    address internal filler = address(0xF111E2);
    address internal exclusive = address(0xE1C1);
    uint256 internal nonce;

    uint256 internal constant IN_AMOUNT = 3000 ether;
    uint256 internal constant OUT_AMOUNT = 1 ether;

    function setUp() public {
        permit2 = ISignatureTransfer(deployPermit2());
        settler = new SameChainSettler(permit2);
        tokenIn = new TokenMock("In", "IN");
        tokenOut = new TokenMock("Out", "OUT");

        swapper = vm.addr(SWAPPER_PK);
        tokenIn.mint(swapper, IN_AMOUNT * 10);
        vm.prank(swapper);
        tokenIn.approve(address(permit2), type(uint256).max);

        tokenOut.mint(filler, OUT_AMOUNT * 10);
        vm.prank(filler);
        tokenOut.approve(address(settler), type(uint256).max);

        tokenOut.mint(exclusive, OUT_AMOUNT * 10);
        vm.prank(exclusive);
        tokenOut.approve(address(settler), type(uint256).max);

        vm.warp(1_000_000);
    }

    // --------------------------------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------------------------------

    function _inner(
        address exclusiveFiller,
        uint32 exclusivityEnds
    )
        internal
        view
        returns (SameChainSettler.SolventOrder memory)
    {
        return SameChainSettler.SolventOrder({
            inputToken: address(tokenIn),
            inputAmount: IN_AMOUNT,
            outputToken: address(tokenOut),
            outputAmount: OUT_AMOUNT,
            recipient: swapper,
            exclusiveFiller: exclusiveFiller,
            exclusivityEnds: exclusivityEnds
        });
    }

    function _order(
        SameChainSettler.SolventOrder memory inner,
        bytes32 orderDataType
    )
        internal
        returns (GaslessCrossChainOrder memory)
    {
        return GaslessCrossChainOrder({
            originSettler: address(settler),
            user: swapper,
            nonce: nonce++,
            originChainId: block.chainid,
            openDeadline: uint32(block.timestamp + 500),
            fillDeadline: uint32(block.timestamp + 1000),
            orderDataType: orderDataType,
            orderData: abi.encode(inner)
        });
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

    function _open(SameChainSettler.SolventOrder memory inner)
        internal
        returns (GaslessCrossChainOrder memory order, bytes32 id)
    {
        order = _order(inner, SOLVENT_ORDER_TYPE_HASH);
        id = settler.orderIdFor(order);
        settler.openFor(order, _sign(order, inner), "");
    }

    // --------------------------------------------------------------------------------------------
    // Open
    // --------------------------------------------------------------------------------------------

    function test_openFor_escrowsTheInput() public {
        uint256 before = tokenIn.balanceOf(swapper);
        (, bytes32 id) = _open(_inner(address(0), 0));

        assertEq(tokenIn.balanceOf(address(settler)), IN_AMOUNT, "settler holds the escrow");
        assertEq(tokenIn.balanceOf(swapper), before - IN_AMOUNT, "swapper paid the input");

        (address token,, bool filled, uint256 amount,) = settler.escrows(id);
        assertEq(token, address(tokenIn));
        assertEq(amount, IN_AMOUNT);
        assertFalse(filled);
    }

    function test_openFor_revertsOnUnsupportedOrderType() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        GaslessCrossChainOrder memory order = _order(inner, keccak256("SomeOtherOrder(uint256 x)"));
        // Sign before arming expectRevert: `_sign` itself calls the settler, and expectRevert binds to
        // the very next external call.
        bytes memory sig = _sign(order, inner);
        vm.expectRevert(
            abi.encodeWithSelector(
                SameChainSettler.UnsupportedOrderType.selector, keccak256("SomeOtherOrder(uint256 x)")
            )
        );
        settler.openFor(order, sig, "");
    }

    function test_openFor_revertsOnWrongSettler() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        GaslessCrossChainOrder memory order = _order(inner, SOLVENT_ORDER_TYPE_HASH);
        order.originSettler = address(0xBAD);
        bytes memory sig = _sign(order, inner);
        vm.expectRevert(
            abi.encodeWithSelector(SameChainSettler.WrongSettler.selector, address(0xBAD), address(settler))
        );
        settler.openFor(order, sig, "");
    }

    // Permit2's unordered nonce bitmap is the replay guard; the settler keeps none of its own.
    function test_openFor_revertsOnReplayedNonce() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        GaslessCrossChainOrder memory order = _order(inner, SOLVENT_ORDER_TYPE_HASH);
        bytes memory sig = _sign(order, inner);
        settler.openFor(order, sig, "");

        vm.expectRevert();
        settler.openFor(order, sig, "");
    }

    // The self-signed path: no Permit2, the user is msg.sender.
    function test_open_onchainPathEscrowsAndFills() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        OnchainCrossChainOrder memory order = OnchainCrossChainOrder({
            fillDeadline: uint32(block.timestamp + 1000),
            orderDataType: SOLVENT_ORDER_TYPE_HASH,
            orderData: abi.encode(inner)
        });

        vm.startPrank(swapper);
        tokenIn.approve(address(settler), IN_AMOUNT);
        settler.open(order);
        vm.stopPrank();

        assertEq(tokenIn.balanceOf(address(settler)), IN_AMOUNT, "escrowed without Permit2");
        assertEq(settler.onchainNonces(swapper), 1, "the opener's counter advanced");
    }

    // --------------------------------------------------------------------------------------------
    // Fill
    // --------------------------------------------------------------------------------------------

    function test_fill_paysTheUserAndReleasesTheEscrow() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        uint256 swapperOutBefore = tokenOut.balanceOf(swapper);

        vm.prank(filler);
        settler.fill(id, order.orderData, "");

        assertEq(tokenOut.balanceOf(swapper), swapperOutBefore + OUT_AMOUNT, "user received the output");
        assertEq(tokenIn.balanceOf(filler), IN_AMOUNT, "filler received the escrow in the same call");
        assertEq(tokenIn.balanceOf(address(settler)), 0, "settler holds nothing after");
    }

    function test_fill_revertsOnSecondFill() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        vm.prank(filler);
        settler.fill(id, order.orderData, "");

        vm.prank(filler);
        vm.expectRevert(abi.encodeWithSelector(SameChainSettler.AlreadyFilled.selector, id));
        settler.fill(id, order.orderData, "");
    }

    function test_fill_revertsAfterFillDeadline() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        vm.warp(block.timestamp + 1001);
        vm.prank(filler);
        vm.expectRevert(abi.encodeWithSelector(SameChainSettler.FillDeadlinePassed.selector, id));
        settler.fill(id, order.orderData, "");
    }

    function test_fill_revertsOnUnknownOrder() public {
        bytes32 id = keccak256("nope");
        vm.prank(filler);
        vm.expectRevert(abi.encodeWithSelector(SameChainSettler.UnknownOrder.selector, id));
        settler.fill(id, abi.encode(_inner(address(0), 0)), "");
    }

    // `originData` is filler-supplied; it must be the bytes that were escrowed, not a cheaper order.
    function test_fill_revertsOnOriginDataMismatch() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        (, bytes32 id) = _open(inner);

        SameChainSettler.SolventOrder memory cheaper = inner;
        cheaper.outputAmount = 1 wei;

        vm.prank(filler);
        vm.expectRevert(abi.encodeWithSelector(SameChainSettler.OriginDataMismatch.selector, id));
        settler.fill(id, abi.encode(cheaper), "");
    }

    function test_fill_revertsForNonExclusiveFillerDuringWindow() public {
        uint32 ends = uint32(block.timestamp + 100);
        SameChainSettler.SolventOrder memory inner = _inner(exclusive, ends);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        vm.prank(filler);
        vm.expectRevert(abi.encodeWithSelector(SameChainSettler.NotExclusiveFiller.selector, filler, exclusive));
        settler.fill(id, order.orderData, "");
    }

    function test_fill_exclusiveFillerMayFillInsideTheWindow() public {
        uint32 ends = uint32(block.timestamp + 100);
        SameChainSettler.SolventOrder memory inner = _inner(exclusive, ends);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        vm.prank(exclusive);
        settler.fill(id, order.orderData, "");
        assertEq(tokenIn.balanceOf(exclusive), IN_AMOUNT, "exclusive filler settled");
    }

    function test_fill_opensToAnyoneAfterTheWindow() public {
        uint32 ends = uint32(block.timestamp + 100);
        SameChainSettler.SolventOrder memory inner = _inner(exclusive, ends);
        (GaslessCrossChainOrder memory order, bytes32 id) = _open(inner);

        vm.warp(uint256(ends) + 1);
        vm.prank(filler);
        settler.fill(id, order.orderData, "");
        assertEq(tokenIn.balanceOf(filler), IN_AMOUNT, "non-exclusive filler settled after the window");
    }

    // --------------------------------------------------------------------------------------------
    // Resolve
    // --------------------------------------------------------------------------------------------

    function test_resolveFor_describesTheOrderWithoutADecoder() public {
        SameChainSettler.SolventOrder memory inner = _inner(address(0), 0);
        GaslessCrossChainOrder memory order = _order(inner, SOLVENT_ORDER_TYPE_HASH);

        ResolvedCrossChainOrder memory r = settler.resolveFor(order, "");

        assertEq(r.user, swapper);
        assertEq(r.orderId, settler.orderIdFor(order));
        assertEq(r.maxSpent.length, 1);
        assertEq(r.maxSpent[0].token, bytes32(uint256(uint160(address(tokenOut)))), "filler spends the output token");
        assertEq(r.maxSpent[0].amount, OUT_AMOUNT);
        assertEq(r.minReceived.length, 1);
        assertEq(r.minReceived[0].token, bytes32(uint256(uint160(address(tokenIn)))), "filler receives the input token");
        assertEq(r.minReceived[0].amount, IN_AMOUNT);
        assertEq(r.fillInstructions.length, 1);
        assertEq(r.fillInstructions[0].destinationChainId, uint64(block.chainid));
        assertEq(
            r.fillInstructions[0].destinationSettler,
            bytes32(uint256(uint160(address(settler)))),
            "same chain: this contract is also the destination settler"
        );
        assertEq(r.fillInstructions[0].originData, order.orderData, "originData is what fill() expects");
    }
}
