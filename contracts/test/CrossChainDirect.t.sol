// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { IERC1271 } from "@openzeppelin/contracts/interfaces/IERC1271.sol";

import { Aqua } from "@1inch/aqua/src/Aqua.sol";
import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { WETH } from "solmate/src/tokens/WETH.sol";

import { TheCompact } from "the-compact/src/TheCompact.sol";
import { AlwaysOKAllocator } from "the-compact/src/test/AlwaysOKAllocator.sol";
import { ResetPeriod } from "the-compact/src/types/ResetPeriod.sol";
import { Scope } from "the-compact/src/types/Scope.sol";
import { Claim } from "the-compact/src/types/Claims.sol";
import { Component } from "the-compact/src/types/Components.sol";

import { CompactOriginSettler } from "../src/CompactOriginSettler.sol";
import { CrossChainAquaApp } from "../src/CrossChainAquaApp.sol";
import { DevToken } from "../src/DevToken.sol";
import {
    DirectMakerQuote,
    RouteKind,
    SolventCrossChainOrder,
    SolventMandate
} from "../src/crosschain/CrossChainTypes.sol";
import { IFillProofVerifier } from "../src/interfaces/IFillProofVerifier.sol";
import { IRepaymentProofVerifier } from "../src/interfaces/IRepaymentProofVerifier.sol";
import { IWETH } from "../src/interfaces/IWETH.sol";
import { IMessageTransmitterV2, ITokenMessengerV2 } from "../src/interfaces/ICctpV2.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";

contract TestFillProofVerifier is IFillProofVerifier {
    function verifyFill(bytes calldata proof) external pure returns (VerifiedFill memory) {
        return abi.decode(proof, (VerifiedFill));
    }
}

contract TestRepaymentProofVerifier is IRepaymentProofVerifier {
    function verifyRepayment(bytes calldata proof) external pure returns (VerifiedRepayment memory) {
        return abi.decode(proof, (VerifiedRepayment));
    }
}

contract AlwaysValid1271Maker is IERC1271 {
    function isValidSignature(bytes32, bytes calldata) external pure returns (bytes4) {
        return IERC1271.isValidSignature.selector;
    }
}

contract RejectEth {
    receive() external payable {
        revert();
    }
}

contract CrossChainDirectTest is Test {
    uint256 internal constant ARBITRUM_CHAIN_ID = 42_161;
    uint256 internal constant BASE_CHAIN_ID = 8453;
    uint256 internal constant MAKER_PK = 0xA11CE;
    uint256 internal constant USER_PK = 0xB0B;
    uint256 internal constant WBTC_DUE = 1e8;
    uint256 internal constant WETH_OUT = 1 ether;
    uint256 internal constant ERC20_OUT = 1000e6;

    Aqua internal baseAqua;
    Aqua internal originAqua;
    WETH internal weth;
    DevToken internal wbtc;
    DevToken internal destinationToken;
    TheCompact internal compact;
    TestFillProofVerifier internal fillVerifier;
    TestRepaymentProofVerifier internal repaymentVerifier;
    CompactOriginSettler internal originSettler;
    CrossChainAquaApp internal destinationApp;
    AlwaysOKAllocator internal allocator;

    address internal maker;
    address internal user;
    bytes32 internal destinationStrategyHash;
    bytes32 internal destinationErc20StrategyHash;
    bytes32 internal originStrategyHash;
    uint256 internal compactId;

    function setUp() public {
        vm.warp(1_000_000);
        maker = vm.addr(MAKER_PK);
        user = vm.addr(USER_PK);

        vm.chainId(ARBITRUM_CHAIN_ID);
        compact = new TheCompact();
        originAqua = new Aqua();
        wbtc = new DevToken("Wrapped Bitcoin", "WBTC", 8);
        allocator = new AlwaysOKAllocator();

        vm.chainId(BASE_CHAIN_ID);
        baseAqua = new Aqua();
        weth = new WETH();
        destinationToken = new DevToken("USD Coin", "USDC", 6);
        fillVerifier = new TestFillProofVerifier();
        repaymentVerifier = new TestRepaymentProofVerifier();

        uint256 deployerNonce = vm.getNonce(address(this));
        address predictedOrigin = vm.computeCreateAddress(address(this), deployerNonce);
        address predictedDestination = vm.computeCreateAddress(address(this), deployerNonce + 1);

        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler = new CompactOriginSettler(
            compact,
            IAqua(address(originAqua)),
            wbtc,
            BASE_CHAIN_ID,
            predictedDestination,
            fillVerifier,
            CompactOriginSettler.RoutedConfig({
                originUsdc: destinationToken,
                swapRouter: ISwapVM(address(1)),
                tokenMessenger: ITokenMessengerV2(address(2)),
                destinationUsdc: address(destinationToken),
                destinationDomain: 6,
                minFinalityThreshold: 2000,
                feeRecipient: address(this)
            })
        );
        assertEq(address(originSettler), predictedOrigin);

        vm.chainId(BASE_CHAIN_ID);
        destinationApp = new CrossChainAquaApp(
            IAqua(address(baseAqua)),
            IWETH(address(weth)),
            ARBITRUM_CHAIN_ID,
            address(originSettler),
            address(compact),
            address(wbtc),
            address(fillVerifier),
            repaymentVerifier,
            CrossChainAquaApp.CctpConfig({
                usdc: destinationToken,
                messageTransmitter: IMessageTransmitterV2(address(3)),
                originDomain: 3,
                destinationDomain: 6,
                originTokenMessenger: bytes32(uint256(2)),
                destinationTokenMessenger: address(4),
                originUsdc: address(destinationToken),
                messageVersion: 1,
                burnMessageVersion: 1,
                minFinalityThreshold: 2000,
                feeRecipient: address(this)
            })
        );
        assertEq(address(destinationApp), predictedDestination);

        (destinationStrategyHash, originStrategyHash) = _configureMaker(maker, 2 * WBTC_DUE, 0, keccak256("maker"));
        destinationErc20StrategyHash =
            _shipDestinationToken(maker, destinationToken, 5000e6, 2 * WBTC_DUE, keccak256("maker-usdc"));
        compactId = _depositCompact(user, WBTC_DUE);
    }

    function test_case1_usesAquaOnBothChainsAndLeavesNoResolverInventory() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        uint256 userEthBefore = user.balance;
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, signature);

        assertEq(user.balance - userEthBefore, WETH_OUT);
        assertEq(weth.balanceOf(address(destinationApp)), 0);
        assertEq(address(destinationApp).balance, 0);
        assertEq(_outstanding(maker, destinationStrategyHash), WBTC_DUE);

        vm.prank(maker);
        destinationApp.configureDirectStrategy(destinationStrategyHash, address(weth), 2 * WBTC_DUE, false);

        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("fill-1"));
        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);

        assertEq(wbtc.balanceOf(maker), WBTC_DUE);
        assertEq(wbtc.balanceOf(address(originSettler)), 0);
        assertEq(wbtc.balanceOf(address(compact)), 0);
        (uint248 credited, uint8 tokensCount) =
            originAqua.rawBalances(maker, address(originSettler), originStrategyHash, address(wbtc));
        assertEq(credited, WBTC_DUE);
        assertEq(tokensCount, 1);

        IRepaymentProofVerifier.VerifiedRepayment memory repayment = IRepaymentProofVerifier.VerifiedRepayment({
            orderId: mandate.orderId,
            originChainId: ARBITRUM_CHAIN_ID,
            originSettler: address(originSettler),
            maker: maker,
            repaymentToken: address(wbtc),
            repaymentAmount: WBTC_DUE,
            repaymentId: keccak256("repayment-1")
        });
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));

        assertEq(_outstanding(maker, destinationStrategyHash), 0);
        (,,,,, CrossChainAquaApp.ReceivableState state) = destinationApp.directReceivables(mandate.orderId);
        assertEq(uint8(state), uint8(CrossChainAquaApp.ReceivableState.Repaid));
    }

    function test_case1_deliversDestinationErc20AndLeavesNoResolverInventory() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenarioForOutput(
            user,
            maker,
            MAKER_PK,
            destinationErc20StrategyHash,
            originStrategyHash,
            1,
            address(destinationToken),
            ERC20_OUT
        );

        uint256 recipientBefore = destinationToken.balanceOf(user);
        uint256 makerBefore = destinationToken.balanceOf(maker);
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, signature);

        assertEq(destinationToken.balanceOf(user) - recipientBefore, ERC20_OUT);
        assertEq(makerBefore - destinationToken.balanceOf(maker), ERC20_OUT);
        assertEq(destinationToken.balanceOf(address(destinationApp)), 0);
        assertEq(weth.balanceOf(address(destinationApp)), 0);
        assertEq(address(destinationApp).balance, 0);
        assertEq(_outstanding(maker, destinationErc20StrategyHash), WBTC_DUE);

        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleDirect(
            order, mandate, abi.encode(_verifiedFill(order, quote, keccak256("fill-erc20"))), compactClaim
        );
        assertEq(wbtc.balanceOf(maker), WBTC_DUE);
        assertEq(wbtc.balanceOf(address(originSettler)), 0);

        IRepaymentProofVerifier.VerifiedRepayment memory repayment = IRepaymentProofVerifier.VerifiedRepayment({
            orderId: mandate.orderId,
            originChainId: ARBITRUM_CHAIN_ID,
            originSettler: address(originSettler),
            maker: maker,
            repaymentToken: address(wbtc),
            repaymentAmount: WBTC_DUE,
            repaymentId: keccak256("repayment-erc20")
        });
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));

        assertEq(_outstanding(maker, destinationErc20StrategyHash), 0);
        (,,,,, CrossChainAquaApp.ReceivableState state) = destinationApp.directReceivables(mandate.orderId);
        assertEq(uint8(state), uint8(CrossChainAquaApp.ReceivableState.Repaid));
    }

    function test_fillDirect_revertsAtomicallyWhenRecipientRejectsEth() public {
        RejectEth recipient = new RejectEth();
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(address(recipient), maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        uint256 makerWethBefore = weth.balanceOf(maker);
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(
            abi.encodeWithSelector(CrossChainAquaApp.EthDeliveryFailed.selector, address(recipient), WETH_OUT)
        );
        destinationApp.fillDirect(order, mandate, quote, signature);

        assertEq(weth.balanceOf(maker), makerWethBefore);
        assertEq(_outstanding(maker, destinationStrategyHash), 0);
        assertFalse(destinationApp.usedOrders(mandate.orderId));
    }

    function test_fillDirect_rejectsBadMakerSignature() public {
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, DirectMakerQuote memory quote,,) =
            _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        bytes memory badSignature = _sign(0xBAD, destinationApp.directQuoteDigest(quote));
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidMakerQuote.selector);
        destinationApp.fillDirect(order, mandate, quote, badSignature);
    }

    function test_fillDirect_rejectsSignatureFromWrongChainDomain() public {
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, DirectMakerQuote memory quote,,) =
            _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.chainId(ARBITRUM_CHAIN_ID);
        bytes memory wrongDomainSignature = _sign(MAKER_PK, destinationApp.directQuoteDigest(quote));
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidMakerQuote.selector);
        destinationApp.fillDirect(order, mandate, quote, wrongDomainSignature);
    }

    function test_fillDirect_rejectsExpiredQuote() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.warp(quote.expires + 1);
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.QuoteExpired.selector, quote.expires));
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_enforcesExclusiveFillerUntilWindowEnds() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        address exclusiveFiller = address(0xF11E);
        order.exclusiveFiller = exclusiveFiller;
        order.exclusivityEnds = uint48(block.timestamp + 1 hours);
        mandate.exclusiveFiller = exclusiveFiller;
        mandate.orderId = destinationApp.orderIdFor(order);
        quote.orderId = mandate.orderId;
        vm.chainId(BASE_CHAIN_ID);
        signature = _sign(MAKER_PK, destinationApp.directQuoteDigest(quote));

        vm.expectRevert(
            abi.encodeWithSelector(CrossChainAquaApp.UnauthorizedFiller.selector, address(this), exclusiveFiller)
        );
        destinationApp.fillDirect(order, mandate, quote, signature);

        vm.prank(exclusiveFiller);
        destinationApp.fillDirect(order, mandate, quote, signature);
        assertEq(user.balance, WETH_OUT);
    }

    function test_fillDirect_rejectsPastFillDeadline() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.warp(order.fillDeadline + 1);
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.FillDeadlinePassed.selector, order.fillDeadline));
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_rejectsMandateThatDoesNotMatchOrder() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        mandate.recipient = address(0xBEEF);

        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidMandate.selector);
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_acceptsErc1271MakerSignature() public {
        AlwaysValid1271Maker contractMaker = new AlwaysValid1271Maker();
        (bytes32 destinationStrategy, bytes32 originStrategy) =
            _configureMaker(address(contractMaker), WBTC_DUE, 0, keccak256("1271"));
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, DirectMakerQuote memory quote,,) =
            _scenario(user, address(contractMaker), 0, destinationStrategy, originStrategy, 1);

        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, hex"1234");
        assertEq(user.balance, WETH_OUT);
        assertEq(_outstanding(address(contractMaker), destinationStrategy), WBTC_DUE);
    }

    function test_fillDirect_rejectsCapacityAboveMakerLimit() public {
        vm.chainId(BASE_CHAIN_ID);
        vm.prank(maker);
        destinationApp.configureDirectStrategy(destinationStrategyHash, address(weth), WBTC_DUE - 1, true);

        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.CapacityExceeded.selector, 0, WBTC_DUE, WBTC_DUE - 1));
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_rejectsStrategyConfiguredForAnotherOutputToken() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationErc20StrategyHash, originStrategyHash, 1);

        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossChainAquaApp.StrategyTokenMismatch.selector, address(destinationToken), address(weth)
            )
        );
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_rejectsQuoteAndOrderReplay() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, signature);
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.OrderAlreadyUsed.selector, mandate.orderId));
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_fillDirect_rejectsQuoteNonceReplayForDifferentOrder() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);

        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, signature);

        order.nonce += 1;
        mandate.orderId = destinationApp.orderIdFor(order);
        quote.orderId = mandate.orderId;
        signature = _sign(MAKER_PK, destinationApp.directQuoteDigest(quote));
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.QuoteNonceAlreadyUsed.selector, maker, quote.nonce));
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function test_settleDirect_usesFallbackAfterMakerDocksOriginStrategy() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        _fill(order, mandate, quote, signature);

        vm.chainId(ARBITRUM_CHAIN_ID);
        address[] memory tokens = _oneAddress(address(wbtc));
        vm.prank(maker);
        originAqua.dock(address(originSettler), originStrategyHash, tokens);
        originSettler.settleDirect(
            order, mandate, abi.encode(_verifiedFill(order, quote, keccak256("fill-fallback"))), compactClaim
        );

        assertEq(wbtc.balanceOf(maker), WBTC_DUE);
        assertEq(wbtc.balanceOf(address(originSettler)), 0);
        (uint248 credited, uint8 tokensCount) =
            originAqua.rawBalances(maker, address(originSettler), originStrategyHash, address(wbtc));
        assertEq(credited, 0);
        assertEq(tokensCount, type(uint8).max);
    }

    function test_settleDirect_revertsCompactClaimWhenActiveAquaPushFails() public {
        bytes32 saturatedOrigin = _shipOrigin(maker, type(uint248).max, keccak256("saturated"));
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, saturatedOrigin, 0);
        _fill(order, mandate, quote, signature);

        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("fill-overflow"));
        vm.chainId(ARBITRUM_CHAIN_ID);
        vm.expectRevert();
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);

        assertEq(compact.balanceOf(user, compactId), WBTC_DUE);
        assertEq(wbtc.balanceOf(address(originSettler)), 0);
        assertFalse(originSettler.settledOrders(mandate.orderId));
        assertFalse(originSettler.usedFillProofs(fill.fillId));
    }

    function test_settleDirect_rejectsFillProofWithWrongRepayment() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        _fill(order, mandate, quote, signature);

        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("fill-bad"));
        fill.repaymentAmount -= 1;
        vm.chainId(ARBITRUM_CHAIN_ID);
        vm.expectRevert(CompactOriginSettler.InvalidFillProof.selector);
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);
        assertEq(compact.balanceOf(user, compactId), WBTC_DUE);
    }

    function test_settleDirect_rejectsClaimWithDifferentMandateWitness() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        _fill(order, mandate, quote, signature);
        compactClaim.witness = keccak256("different mandate");
        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("fill-witness"));

        vm.chainId(ARBITRUM_CHAIN_ID);
        vm.expectRevert(CompactOriginSettler.InvalidCompactClaim.selector);
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);
        assertEq(compact.balanceOf(user, compactId), WBTC_DUE);
    }

    function test_settleDirect_rejectsReplayWithoutMovingFunds() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        _fill(order, mandate, quote, signature);
        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("fill-replay"));

        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);
        uint256 makerBalance = wbtc.balanceOf(maker);
        vm.expectRevert(abi.encodeWithSelector(CompactOriginSettler.OrderAlreadySettled.selector, mandate.orderId));
        originSettler.settleDirect(order, mandate, abi.encode(fill), compactClaim);
        assertEq(wbtc.balanceOf(maker), makerBalance);
    }

    function test_confirmDirectRepayment_rejectsForgedAndDuplicateProofs() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        ) = _scenario(user, maker, MAKER_PK, destinationStrategyHash, originStrategyHash, 0);
        _fill(order, mandate, quote, signature);
        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleDirect(
            order, mandate, abi.encode(_verifiedFill(order, quote, keccak256("fill-close"))), compactClaim
        );

        IRepaymentProofVerifier.VerifiedRepayment memory repayment = IRepaymentProofVerifier.VerifiedRepayment({
            orderId: mandate.orderId,
            originChainId: ARBITRUM_CHAIN_ID,
            originSettler: address(originSettler),
            maker: maker,
            repaymentToken: address(destinationToken),
            repaymentAmount: WBTC_DUE,
            repaymentId: keccak256("repayment-close")
        });
        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidRepaymentProof.selector);
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));
        assertEq(_outstanding(maker, destinationStrategyHash), WBTC_DUE);

        repayment.repaymentToken = address(wbtc);
        repayment.repaymentAmount = WBTC_DUE - 1;
        vm.expectRevert(CrossChainAquaApp.InvalidRepaymentProof.selector);
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));

        repayment.repaymentAmount = WBTC_DUE;
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));
        vm.expectRevert(
            abi.encodeWithSelector(CrossChainAquaApp.RepaymentProofAlreadyUsed.selector, keccak256("repayment-close"))
        );
        destinationApp.confirmDirectRepayment(mandate.orderId, abi.encode(repayment));
    }

    function _scenario(
        address recipient,
        address quoteMaker,
        uint256 makerPrivateKey,
        bytes32 destinationStrategy,
        bytes32 originStrategy,
        uint256 quoteNonce
    )
        internal
        returns (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        )
    {
        return _scenarioForOutput(
            recipient,
            quoteMaker,
            makerPrivateKey,
            destinationStrategy,
            originStrategy,
            quoteNonce,
            address(0),
            WETH_OUT
        );
    }

    function _scenarioForOutput(
        address recipient,
        address quoteMaker,
        uint256 makerPrivateKey,
        bytes32 destinationStrategy,
        bytes32 originStrategy,
        uint256 quoteNonce,
        address outputToken,
        uint256 outputAmount
    )
        internal
        returns (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            DirectMakerQuote memory quote,
            bytes memory signature,
            Claim memory compactClaim
        )
    {
        order = SolventCrossChainOrder({
            user: user,
            nonce: 7,
            originChainId: ARBITRUM_CHAIN_ID,
            originSettler: address(originSettler),
            compact: address(compact),
            compactId: compactId,
            compactExpires: block.timestamp + 2 days,
            inputToken: address(wbtc),
            inputAmount: WBTC_DUE,
            destinationChainId: BASE_CHAIN_ID,
            outputToken: outputToken,
            minimumOutputAmount: outputAmount,
            recipient: recipient,
            destinationSettler: address(destinationApp),
            fillProofVerifier: address(fillVerifier),
            exclusiveFiller: address(0),
            exclusivityEnds: 0,
            fillDeadline: uint48(block.timestamp + 1 days),
            routeKind: RouteKind.DirectMaker
        });
        bytes32 orderId = destinationApp.orderIdFor(order);
        mandate = SolventMandate({
            orderId: orderId,
            destinationChainId: BASE_CHAIN_ID,
            destinationSettler: address(destinationApp),
            fillProofVerifier: address(fillVerifier),
            outputToken: outputToken,
            minimumOutputAmount: outputAmount,
            recipient: recipient,
            fillDeadline: order.fillDeadline,
            exclusiveFiller: address(0),
            routeKind: RouteKind.DirectMaker
        });
        quote = DirectMakerQuote({
            orderId: orderId,
            maker: quoteMaker,
            destinationStrategyHash: destinationStrategy,
            originStrategyHash: originStrategy,
            outputAmount: outputAmount,
            repaymentAmount: WBTC_DUE,
            nonce: quoteNonce,
            expires: uint48(block.timestamp + 1 hours)
        });

        vm.chainId(BASE_CHAIN_ID);
        bytes32 quoteDigest = destinationApp.directQuoteDigest(quote);
        signature = makerPrivateKey == 0 ? bytes("") : _sign(makerPrivateKey, quoteDigest);

        vm.chainId(ARBITRUM_CHAIN_ID);
        compactClaim = _registerClaim(order, mandate);
    }

    function _registerClaim(
        SolventCrossChainOrder memory order,
        SolventMandate memory mandate
    )
        internal
        returns (Claim memory compactClaim)
    {
        Component[] memory claimants = new Component[](1);
        claimants[0] = Component({ claimant: uint256(uint160(address(originSettler))), amount: WBTC_DUE });
        compactClaim = Claim({
            allocatorData: "",
            sponsorSignature: "",
            sponsor: user,
            nonce: 91,
            expires: order.compactExpires,
            witness: originSettler.mandateHash(mandate),
            witnessTypestring: originSettler.MANDATE_WITNESS_TYPESTRING(),
            id: compactId,
            allocatedAmount: WBTC_DUE,
            claimants: claimants
        });

        bytes32 claimHash = keccak256(
            abi.encode(
                originSettler.COMPACT_WITH_MANDATE_TYPEHASH(),
                address(originSettler),
                user,
                compactClaim.nonce,
                compactClaim.expires,
                bytes12(bytes32(compactId)),
                address(wbtc),
                WBTC_DUE,
                compactClaim.witness
            )
        );
        bytes32 compactTypehash = originSettler.COMPACT_WITH_MANDATE_TYPEHASH();
        vm.prank(user);
        compact.register(claimHash, compactTypehash);
    }

    function _verifiedFill(
        SolventCrossChainOrder memory order,
        DirectMakerQuote memory quote,
        bytes32 fillId
    )
        internal
        view
        returns (IFillProofVerifier.VerifiedFill memory)
    {
        return IFillProofVerifier.VerifiedFill({
            orderId: quote.orderId,
            routeKind: RouteKind.DirectMaker,
            destinationChainId: BASE_CHAIN_ID,
            destinationApp: address(destinationApp),
            recipient: order.recipient,
            outputToken: order.outputToken,
            outputAmount: quote.outputAmount,
            destinationMaker: quote.maker,
            destinationStrategyHash: quote.destinationStrategyHash,
            originStrategyHash: quote.originStrategyHash,
            repaymentToken: address(wbtc),
            repaymentAmount: quote.repaymentAmount,
            maxCctpFee: 0,
            makerQuoteHash: destinationApp.directQuoteDigest(quote),
            fillId: fillId
        });
    }

    function _fill(
        SolventCrossChainOrder memory order,
        SolventMandate memory mandate,
        DirectMakerQuote memory quote,
        bytes memory signature
    )
        internal
    {
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillDirect(order, mandate, quote, signature);
    }

    function _configureMaker(
        address makerAddress,
        uint256 maximumWbtc,
        uint248 initialOriginCredit,
        bytes32 salt
    )
        internal
        returns (bytes32 destinationStrategy, bytes32 originStrategy)
    {
        vm.chainId(BASE_CHAIN_ID);
        vm.deal(makerAddress, 10 ether);
        vm.prank(makerAddress);
        weth.deposit{ value: 5 ether }();
        vm.prank(makerAddress);
        weth.approve(address(baseAqua), type(uint256).max);

        address[] memory baseTokens = _oneAddress(address(weth));
        uint256[] memory baseAmounts = _oneUint(5 ether);
        vm.prank(makerAddress);
        destinationStrategy =
            baseAqua.ship(address(destinationApp), abi.encode("direct-base", salt), baseTokens, baseAmounts);
        vm.prank(makerAddress);
        destinationApp.configureDirectStrategy(destinationStrategy, address(weth), maximumWbtc, true);

        originStrategy = _shipOrigin(makerAddress, initialOriginCredit, salt);
    }

    function _shipDestinationToken(
        address makerAddress,
        DevToken token,
        uint256 amount,
        uint256 maximumRepayment,
        bytes32 salt
    )
        internal
        returns (bytes32 strategy)
    {
        vm.chainId(BASE_CHAIN_ID);
        token.mint(makerAddress, amount);
        vm.prank(makerAddress);
        token.approve(address(baseAqua), type(uint256).max);

        address[] memory tokens = _oneAddress(address(token));
        uint256[] memory amounts = _oneUint(amount);
        vm.prank(makerAddress);
        strategy = baseAqua.ship(address(destinationApp), abi.encode("direct-token", salt), tokens, amounts);
        vm.prank(makerAddress);
        destinationApp.configureDirectStrategy(strategy, address(token), maximumRepayment, true);
    }

    function _shipOrigin(address makerAddress, uint248 initialCredit, bytes32 salt)
        internal
        returns (bytes32 strategy)
    {
        vm.chainId(ARBITRUM_CHAIN_ID);
        address[] memory tokens = _oneAddress(address(wbtc));
        uint256[] memory amounts = _oneUint(initialCredit);
        vm.prank(makerAddress);
        strategy = originAqua.ship(address(originSettler), abi.encode("direct-origin", salt), tokens, amounts);
    }

    function _depositCompact(address sponsor, uint256 amount) internal returns (uint256 id) {
        vm.chainId(ARBITRUM_CHAIN_ID);
        uint96 allocatorId = compact.__registerAllocator(address(allocator), "");
        bytes12 lockTag = bytes12(
            bytes32(
                (uint256(Scope.Multichain) << 255) | (uint256(ResetPeriod.TenMinutes) << 252)
                    | (uint256(allocatorId) << 160)
            )
        );
        wbtc.mint(sponsor, amount);
        vm.prank(sponsor);
        wbtc.approve(address(compact), amount);
        vm.prank(sponsor);
        id = compact.depositERC20(address(wbtc), lockTag, amount, sponsor);
    }

    function _outstanding(address makerAddress, bytes32 strategyHash) internal view returns (uint256 outstanding) {
        (,, outstanding,) = destinationApp.directStrategies(makerAddress, strategyHash);
    }

    function _sign(uint256 privateKey, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return bytes.concat(r, s, bytes1(v));
    }

    function _oneAddress(address value) internal pure returns (address[] memory values) {
        values = new address[](1);
        values[0] = value;
    }

    function _oneUint(uint256 value) internal pure returns (uint256[] memory values) {
        values = new uint256[](1);
        values[0] = value;
    }
}
