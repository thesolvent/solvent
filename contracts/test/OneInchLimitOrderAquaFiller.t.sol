// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";
import { IOrderMixin } from "@1inch/limit-order-protocol-contract/contracts/interfaces/IOrderMixin.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

import { OneInchLimitOrderAquaFiller } from "../src/OneInchLimitOrderAquaFiller.sol";
import { OneInchLimitOrderAquaFillerHarness } from "./OneInchLimitOrderAquaFillerHarness.sol";

/// @notice Hermetic suite: everything (protocol, Aqua/SwapVM, tokens) is deployed locally from source.
contract OneInchLimitOrderAquaFillerTest is OneInchLimitOrderAquaFillerHarness {
    function setUp() public override {
        _deploy();
    }

    // --------------------------------------------------------------------------------------------
    // Happy path
    // --------------------------------------------------------------------------------------------

    function test_fill_singleMaker_sourcesOutputAndKeepsSpread() public {
        uint256 out = 1 ether;
        uint256 makingAmount = 3100 ether; // what the signer gives us (tokenA)

        ISwapVM.Order memory makerOrder = _shipXycMaker(maker, tokenA, tokenB, 3_000_000 ether, 1000 ether, 10 ether);
        (IOrderMixin.Order memory order, bytes32 r, bytes32 vs) = _signPlainOrder(tokenA, tokenB, makingAmount, out);

        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](1);
        sources[0] = _source(makerOrder, tokenA, tokenB, out, makingAmount);

        filler.fill(order, r, vs, makingAmount, MAKER_AMOUNT_FLAG, "", sources);

        uint256 paidToMaker = tokenA.balanceOf(maker); // Aqua maker starts with 0 tokenA
        assertEq(tokenB.balanceOf(signer), out, "signer receives the taker asset");
        assertEq(tokenA.balanceOf(signer), 0, "signer paid the full making amount");
        assertEq(tokenB.balanceOf(address(filler)), 0, "filler holds no output inventory");
        assertEq(tokenA.balanceOf(address(filler)), makingAmount - paidToMaker, "filler keeps the spread");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread is positive");
    }

    function test_fill_multiMaker_sourcesRemainder() public {
        uint256 makingAmount = 3100 ether;
        address makerA = maker;
        address makerB = vm.addr(0x4444);

        ISwapVM.Order memory orderA = _shipXycMaker(makerA, tokenA, tokenB, 1_200_000 ether, 400 ether, 1 ether);
        ISwapVM.Order memory orderB = _shipXycMaker(makerB, tokenA, tokenB, 1_800_000 ether, 600 ether, 1 ether);

        (IOrderMixin.Order memory order, bytes32 r, bytes32 vs) = _signPlainOrder(tokenA, tokenB, makingAmount, 1 ether);

        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](2);
        sources[0] = _source(orderA, tokenA, tokenB, 0.4 ether, makingAmount);
        sources[1] = _source(orderB, tokenA, tokenB, 0.6 ether, makingAmount);

        filler.fill(order, r, vs, makingAmount, MAKER_AMOUNT_FLAG, "", sources);

        assertEq(tokenB.balanceOf(signer), 1 ether, "signer receives the full taker asset");
        assertEq(tokenB.balanceOf(makerA), 0.6 ether, "maker A supplied 0.4");
        assertEq(tokenB.balanceOf(makerB), 0.4 ether, "maker B supplied the 0.6 remainder");
        assertGt(tokenA.balanceOf(address(filler)), 0, "spread retained");
    }

    // --------------------------------------------------------------------------------------------
    // Guards
    // --------------------------------------------------------------------------------------------

    function test_fill_revertsForNonOwner() public {
        (IOrderMixin.Order memory order, bytes32 r, bytes32 vs) = _signPlainOrder(tokenA, tokenB, 1 ether, 1 ether);
        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](0);

        vm.prank(address(0xBEEF));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBEEF)));
        filler.fill(order, r, vs, 1 ether, MAKER_AMOUNT_FLAG, "", sources);
    }

    function test_fill_revertsWhenTakerTraitsBaseSetsAStructuralBit() public {
        (IOrderMixin.Order memory order, bytes32 r, bytes32 vs) = _signPlainOrder(tokenA, tokenB, 1 ether, 1 ether);
        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](0);

        // Bit 251 is `_ARGS_HAS_TARGET`, a structural bit this contract owns.
        uint256 pollutedBase = MAKER_AMOUNT_FLAG | (uint256(1) << 251);
        vm.expectRevert(OneInchLimitOrderAquaFiller.StructuralTraitsBitsSet.selector);
        filler.fill(order, r, vs, 1 ether, pollutedBase, "", sources);
    }

    function test_takerInteraction_revertsWhenCalledDirectly() public {
        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](0);
        (IOrderMixin.Order memory order,,) = _signPlainOrder(tokenA, tokenB, 1 ether, 1 ether);

        vm.expectRevert(
            abi.encodeWithSelector(OneInchLimitOrderAquaFiller.CallbackUnauthorized.selector, address(this))
        );
        filler.takerInteraction(order, "", bytes32(0), address(this), 1 ether, 1 ether, 0, abi.encode(sources));
    }

    function test_takerInteraction_revertsWhenProtocolCallsOutsideAFill() public {
        // Even the real protocol calling us is unauthorized outside a `fill()` we ourselves triggered
        // (`_fillInFlight` guards against a third party naming us as the interaction target on their
        // own, unrelated order).
        OneInchLimitOrderAquaFiller.SourceSwap[] memory sources = new OneInchLimitOrderAquaFiller.SourceSwap[](0);
        (IOrderMixin.Order memory order,,) = _signPlainOrder(tokenA, tokenB, 1 ether, 1 ether);

        vm.prank(address(protocol));
        vm.expectRevert(
            abi.encodeWithSelector(OneInchLimitOrderAquaFiller.CallbackUnauthorized.selector, address(protocol))
        );
        filler.takerInteraction(order, "", bytes32(0), address(protocol), 1 ether, 1 ether, 0, abi.encode(sources));
    }

    function test_sweep_onlyOwner() public {
        tokenA.mint(address(filler), 5 ether);
        filler.sweep(address(tokenA), address(this));
        assertEq(tokenA.balanceOf(address(this)), 5 ether);

        vm.prank(address(0xBEEF));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBEEF)));
        filler.sweep(address(tokenA), address(0xBEEF));
    }
}
