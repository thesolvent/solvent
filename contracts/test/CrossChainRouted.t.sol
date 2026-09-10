// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

import { Aqua } from "@1inch/aqua/src/Aqua.sol";
import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { WETH } from "solmate/src/tokens/WETH.sol";

import { TheCompact } from "the-compact/src/TheCompact.sol";
import { AlwaysOKAllocator } from "the-compact/src/test/AlwaysOKAllocator.sol";
import { Claim } from "the-compact/src/types/Claims.sol";
import { Component } from "the-compact/src/types/Components.sol";
import { ResetPeriod } from "the-compact/src/types/ResetPeriod.sol";
import { Scope } from "the-compact/src/types/Scope.sol";

import { CompactOriginSettler } from "../src/CompactOriginSettler.sol";
import { CrossChainAquaApp } from "../src/CrossChainAquaApp.sol";
import { DevToken } from "../src/DevToken.sol";
import { CctpMessageLib } from "../src/crosschain/CctpMessageLib.sol";
import {
    MakerCreditQuote,
    RouteKind,
    SolventCrossChainOrder,
    SolventMandate
} from "../src/crosschain/CrossChainTypes.sol";
import { ProofKind } from "../src/crosschain/ProofTypes.sol";
import { IMessageTransmitterV2, ITokenMessengerV2 } from "../src/interfaces/ICctpV2.sol";
import { IFillProofVerifier } from "../src/interfaces/IFillProofVerifier.sol";
import { IProofOutbox } from "../src/interfaces/IProofOutbox.sol";
import { IRepaymentProofVerifier } from "../src/interfaces/IRepaymentProofVerifier.sol";
import { IWETH } from "../src/interfaces/IWETH.sol";

interface IMintableToken {
    function mint(address to, uint256 amount) external;
}

contract RoutedFillProofVerifier is IFillProofVerifier {
    function verifyFill(bytes calldata proof) external pure returns (VerifiedFill memory) {
        return abi.decode(proof, (VerifiedFill));
    }
}

contract UnusedRepaymentProofVerifier is IRepaymentProofVerifier {
    function verifyRepayment(bytes calldata proof) external pure returns (VerifiedRepayment memory) {
        return abi.decode(proof, (VerifiedRepayment));
    }
}

contract RoutedProofOutbox is IProofOutbox {
    mapping(bytes32 orderId => bytes32 payloadHash) public payloads;

    function record(bytes32 orderId, ProofKind kind, bytes calldata payload) external returns (bytes32 payloadHash) {
        payloadHash = keccak256(abi.encode(uint8(1), kind, payload));
        payloads[orderId] = payloadHash;
    }
}

contract MockTokenMessengerV2 is ITokenMessengerV2 {
    using SafeERC20 for IERC20;

    address public immutable BURN_SINK;
    bool public failBurn;
    uint256 public lastAmount;
    uint32 public lastDestinationDomain;
    bytes32 public lastMintRecipient;
    bytes32 public lastDestinationCaller;
    uint256 public lastMaxFee;
    uint32 public lastMinFinalityThreshold;
    bytes public lastHookData;

    error BurnFailed();

    constructor(address burnSink) {
        BURN_SINK = burnSink;
    }

    function setFailBurn(bool fail) external {
        failBurn = fail;
    }

    function depositForBurnWithHook(
        uint256 amount,
        uint32 destinationDomain,
        bytes32 mintRecipient,
        address burnToken,
        bytes32 destinationCaller,
        uint256 maxFee,
        uint32 minFinalityThreshold,
        bytes calldata hookData
    )
        external
    {
        require(!failBurn, BurnFailed());
        IERC20(burnToken).safeTransferFrom(msg.sender, BURN_SINK, amount);
        lastAmount = amount;
        lastDestinationDomain = destinationDomain;
        lastMintRecipient = mintRecipient;
        lastDestinationCaller = destinationCaller;
        lastMaxFee = maxFee;
        lastMinFinalityThreshold = minFinalityThreshold;
        lastHookData = hookData;
    }
}

contract MockMessageTransmitterV2 is IMessageTransmitterV2 {
    IMintableToken public immutable USDC;
    bool public failReceive;
    uint256 public mintBonus;
    mapping(bytes32 nonce => bool) public usedNonces;

    constructor(IMintableToken usdc) {
        USDC = usdc;
    }

    function setFailReceive(bool fail) external {
        failReceive = fail;
    }

    function setMintBonus(uint256 bonus) external {
        mintBonus = bonus;
    }

    function receiveMessage(bytes calldata message, bytes calldata) external returns (bool success) {
        if (failReceive) {
            return false;
        }
        CctpMessageLib.ParsedMessage memory parsed = CctpMessageLib.parse(message);
        require(!usedNonces[parsed.nonce]);
        usedNonces[parsed.nonce] = true;
        USDC.mint(address(uint160(uint256(parsed.mintRecipient))), parsed.amount - parsed.feeExecuted + mintBonus);
        return true;
    }
}

contract CrossChainRoutedTest is AquaStrategyBuilders {
    uint256 internal constant ARBITRUM_CHAIN_ID = 42_161;
    uint256 internal constant BASE_CHAIN_ID = 8453;
    uint32 internal constant ARBITRUM_CCTP_DOMAIN = 3;
    uint32 internal constant BASE_CCTP_DOMAIN = 6;
    uint32 internal constant CCTP_FINALITY = 2000;
    uint256 internal constant USER_PK = 0xB0B;
    uint256 internal constant M2_PK = 0xC0FFEE;
    uint256 internal constant INPUT_AMOUNT = 1 ether;
    uint256 internal constant OUTPUT_AMOUNT = 1 ether;
    uint256 internal constant USDC_DUE = 1000 ether;
    uint256 internal constant MAX_CCTP_FEE = 10 ether;
    uint256 internal constant CREDIT_CAP = 5000 ether;

    Aqua internal baseAqua;
    AquaSwapVMRouter internal swapVm;
    TheCompact internal compact;
    AlwaysOKAllocator internal allocator;
    WETH internal weth;
    DevToken internal baseUsdc;
    RoutedFillProofVerifier internal fillVerifier;
    UnusedRepaymentProofVerifier internal repaymentVerifier;
    RoutedProofOutbox internal proofOutbox;
    MockTokenMessengerV2 internal tokenMessenger;
    MockMessageTransmitterV2 internal messageTransmitter;
    CompactOriginSettler internal originSettler;
    CrossChainAquaApp internal destinationApp;
    ISwapVM.Order internal makerOrder;

    address internal user;
    address internal m2;
    address internal feeRecipient = address(0xFEE);
    address internal burnSink = address(0xB0A7);
    address internal destinationTokenMessenger = address(0xC7C7);
    bytes32 internal m2StrategyHash;
    bytes32 internal m1StrategyHash;
    uint256 internal compactId;

    constructor() AquaStrategyBuilders(address(aqua)) { }

    function setUp() public override {
        super.setUp();
        vm.warp(1_000_000);
        user = vm.addr(USER_PK);
        m2 = vm.addr(M2_PK);

        vm.chainId(ARBITRUM_CHAIN_ID);
        compact = new TheCompact();
        allocator = new AlwaysOKAllocator();
        swapVm = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        tokenMessenger = new MockTokenMessengerV2(burnSink);
        fillVerifier = new RoutedFillProofVerifier();

        makerOrder = createStrategy(
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
        m1StrategyHash = shipStrategy(swapVm, makerOrder, tokenA, tokenB, 1000 ether, 2_000_000 ether);
        tokenB.mint(maker, 10_000 ether);

        vm.chainId(BASE_CHAIN_ID);
        baseAqua = new Aqua();
        weth = new WETH();
        baseUsdc = new DevToken("Base USDC", "USDC", 18);
        repaymentVerifier = new UnusedRepaymentProofVerifier();
        proofOutbox = new RoutedProofOutbox();
        messageTransmitter = new MockMessageTransmitterV2(IMintableToken(address(baseUsdc)));

        uint256 deployerNonce = vm.getNonce(address(this));
        address predictedOrigin = vm.computeCreateAddress(address(this), deployerNonce);
        address predictedDestination = vm.computeCreateAddress(address(this), deployerNonce + 1);

        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler = new CompactOriginSettler(
            compact,
            IAqua(address(aqua)),
            tokenA,
            BASE_CHAIN_ID,
            predictedDestination,
            fillVerifier,
            proofOutbox,
            CompactOriginSettler.RoutedConfig({
                originUsdc: tokenB,
                swapRouter: ISwapVM(address(swapVm)),
                tokenMessenger: tokenMessenger,
                destinationUsdc: address(baseUsdc),
                destinationDomain: BASE_CCTP_DOMAIN,
                minFinalityThreshold: CCTP_FINALITY,
                feeRecipient: feeRecipient
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
            address(tokenA),
            address(fillVerifier),
            proofOutbox,
            repaymentVerifier,
            CrossChainAquaApp.CctpConfig({
                usdc: baseUsdc,
                messageTransmitter: messageTransmitter,
                originDomain: ARBITRUM_CCTP_DOMAIN,
                destinationDomain: BASE_CCTP_DOMAIN,
                originTokenMessenger: CctpMessageLib.addressToBytes32(address(tokenMessenger)),
                destinationTokenMessenger: destinationTokenMessenger,
                originUsdc: address(tokenB),
                messageVersion: 1,
                burnMessageVersion: 1,
                minFinalityThreshold: CCTP_FINALITY,
                feeRecipient: feeRecipient
            })
        );
        assertEq(address(destinationApp), predictedDestination);
        assertLe(address(destinationApp).code.length, 24_576);

        _configureM2();
        compactId = _depositCompact();
    }

    function test_case2_routesThroughBothAquaStrategiesAndLeavesNoOrchestratorInventory() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);

        vm.chainId(BASE_CHAIN_ID);
        uint256 userEthBefore = user.balance;
        destinationApp.fillCredit(order, mandate, quote, _sign(M2_PK, destinationApp.creditQuoteDigest(quote)));
        assertEq(user.balance - userEthBefore, OUTPUT_AMOUNT);
        assertEq(_creditOutstanding(), USDC_DUE);

        IFillProofVerifier.VerifiedFill memory fill = _verifiedFill(order, quote, keccak256("case-2-fill"));
        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleRouted(order, mandate, abi.encode(fill), claim, makerOrder);

        assertEq(tokenA.balanceOf(address(originSettler)), 0);
        assertEq(tokenB.balanceOf(address(originSettler)), 0);
        assertEq(tokenA.balanceOf(address(swapVm)), 0);
        assertEq(tokenB.balanceOf(address(swapVm)), 0);
        assertEq(tokenA.balanceOf(address(compact)), 0);
        assertEq(tokenA.balanceOf(maker), INPUT_AMOUNT);
        assertEq(tokenMessenger.lastAmount(), USDC_DUE + MAX_CCTP_FEE);
        assertEq(tokenMessenger.lastDestinationDomain(), BASE_CCTP_DOMAIN);
        assertEq(tokenMessenger.lastMintRecipient(), CctpMessageLib.addressToBytes32(address(destinationApp)));
        assertEq(tokenMessenger.lastDestinationCaller(), CctpMessageLib.addressToBytes32(address(destinationApp)));
        assertEq(tokenMessenger.lastHookData(), CctpMessageLib.repaymentHook(mandate.orderId));
        (uint248 m1InputCredit, uint8 m1TokenCount) =
            aqua.rawBalances(maker, address(swapVm), m1StrategyHash, address(tokenA));
        assertEq(m1InputCredit, 1001 ether);
        assertEq(m1TokenCount, 2);

        vm.chainId(BASE_CHAIN_ID);
        destinationApp.completeRepayment(
            mandate.orderId, _message(mandate.orderId, 3 ether, address(destinationApp)), hex"a77e57"
        );

        assertEq(_creditOutstanding(), 0);
        (uint248 m2UsdcCredit, uint8 m2TokenCount) =
            baseAqua.rawBalances(m2, address(destinationApp), m2StrategyHash, address(baseUsdc));
        assertEq(m2UsdcCredit, USDC_DUE);
        assertEq(m2TokenCount, 2);
        assertEq(baseUsdc.balanceOf(m2), USDC_DUE);
        assertEq(baseUsdc.balanceOf(feeRecipient), MAX_CCTP_FEE - 3 ether);
        assertEq(baseUsdc.balanceOf(address(destinationApp)), 0);
        assertEq(weth.balanceOf(address(destinationApp)), 0);
        assertEq(address(destinationApp).balance, 0);
    }

    function test_fillCredit_deliversDestinationErc20DirectlyFromM2Aqua() public {
        vm.chainId(BASE_CHAIN_ID);
        DevToken destinationToken = new DevToken("Destination Token", "DST", 18);
        destinationToken.mint(m2, 5000 ether);
        vm.prank(m2);
        destinationToken.approve(address(baseAqua), type(uint256).max);

        address[] memory tokens = new address[](2);
        tokens[0] = address(destinationToken);
        tokens[1] = address(baseUsdc);
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 5000 ether;
        vm.prank(m2);
        bytes32 strategyHash = baseAqua.ship(address(destinationApp), abi.encode("m2-erc20"), tokens, amounts);
        vm.prank(m2);
        destinationApp.configureCreditStrategy(strategyHash, address(destinationToken), CREDIT_CAP, true);

        (SolventCrossChainOrder memory order, SolventMandate memory mandate, MakerCreditQuote memory quote,) =
            _scenario(USDC_DUE);
        order.nonce += 1;
        order.outputToken = address(destinationToken);
        order.minimumOutputAmount = 2500 ether;
        mandate.outputToken = address(destinationToken);
        mandate.minimumOutputAmount = order.minimumOutputAmount;
        mandate.orderId = destinationApp.orderIdFor(order);
        quote.orderId = mandate.orderId;
        quote.destinationStrategyHash = strategyHash;
        quote.outputAmount = order.minimumOutputAmount;
        quote.nonce += 1;
        vm.chainId(BASE_CHAIN_ID);
        bytes memory signature = _sign(M2_PK, destinationApp.creditQuoteDigest(quote));

        destinationApp.fillCredit(order, mandate, quote, signature);

        assertEq(destinationToken.balanceOf(user), order.minimumOutputAmount);
        assertEq(destinationToken.balanceOf(address(destinationApp)), 0);
        (,, uint256 outstanding,) = destinationApp.creditStrategies(m2, strategyHash);
        assertEq(outstanding, USDC_DUE);
    }

    function test_settleRouted_rollsBackCompactClaimAndSwapOnSlippage() public {
        uint256 impossibleDue = 3000 ether;
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(impossibleDue);
        _fillCredit(order, mandate, quote);

        vm.chainId(ARBITRUM_CHAIN_ID);
        bytes memory fillProof = abi.encode(_verifiedFill(order, quote, keccak256("slippage")));
        vm.expectRevert();
        originSettler.settleRouted(order, mandate, fillProof, claim, makerOrder);

        assertEq(tokenA.balanceOf(address(compact)), INPUT_AMOUNT);
        assertEq(tokenA.balanceOf(address(originSettler)), 0);
        assertEq(tokenB.balanceOf(address(originSettler)), 0);
        assertFalse(originSettler.settledOrders(mandate.orderId));
        (uint248 m1InputCredit,) = aqua.rawBalances(maker, address(swapVm), m1StrategyHash, address(tokenA));
        assertEq(m1InputCredit, 1000 ether);
    }

    function test_settleRouted_rollsBackClaimAndSwapWhenCctpBurnFails() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillCredit(order, mandate, quote);
        tokenMessenger.setFailBurn(true);

        vm.chainId(ARBITRUM_CHAIN_ID);
        bytes memory fillProof = abi.encode(_verifiedFill(order, quote, keccak256("burn-fail")));
        vm.expectRevert(MockTokenMessengerV2.BurnFailed.selector);
        originSettler.settleRouted(order, mandate, fillProof, claim, makerOrder);

        assertEq(tokenA.balanceOf(address(compact)), INPUT_AMOUNT);
        assertEq(tokenA.balanceOf(address(originSettler)), 0);
        assertEq(tokenB.balanceOf(address(originSettler)), 0);
        assertFalse(originSettler.settledOrders(mandate.orderId));
        (uint248 m1InputCredit,) = aqua.rawBalances(maker, address(swapVm), m1StrategyHash, address(tokenA));
        assertEq(m1InputCredit, 1000 ether);
    }

    function test_completeRepayment_rejectsWrongMessageBindingBeforeMint() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("wrong-binding"));

        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidCctpMessage.selector);
        destinationApp.completeRepayment(
            mandate.orderId, _message(mandate.orderId, 3 ether, address(0xBAD)), hex"a77e57"
        );
        assertEq(baseUsdc.balanceOf(address(destinationApp)), 0);
        assertEq(_creditOutstanding(), USDC_DUE);
    }

    function test_completeRepayment_rejectsFeeAboveSignedBound() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("fee-bound"));

        vm.chainId(BASE_CHAIN_ID);
        vm.expectRevert(CrossChainAquaApp.InvalidCctpMessage.selector);
        destinationApp.completeRepayment(
            mandate.orderId, _message(mandate.orderId, MAX_CCTP_FEE + 1, address(destinationApp)), hex"a77e57"
        );
        assertEq(_creditOutstanding(), USDC_DUE);
    }

    function test_completeRepayment_preservesDonatedUsdcAndRoutesOnlyMintDelta() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("donation"));

        vm.chainId(BASE_CHAIN_ID);
        baseUsdc.mint(address(destinationApp), 77 ether);
        destinationApp.completeRepayment(
            mandate.orderId, _message(mandate.orderId, 4 ether, address(destinationApp)), hex"a77e57"
        );

        assertEq(baseUsdc.balanceOf(address(destinationApp)), 77 ether);
        assertEq(baseUsdc.balanceOf(m2), USDC_DUE);
        assertEq(baseUsdc.balanceOf(feeRecipient), MAX_CCTP_FEE - 4 ether);
    }

    function test_completeRepayment_retriesAfterTransmitterFailureAndRejectsReplay() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("retry"));
        bytes memory message = _message(mandate.orderId, 2 ether, address(destinationApp));

        vm.chainId(BASE_CHAIN_ID);
        messageTransmitter.setFailReceive(true);
        vm.expectRevert(CrossChainAquaApp.CctpReceiveFailed.selector);
        destinationApp.completeRepayment(mandate.orderId, message, hex"a77e57");
        assertEq(_creditOutstanding(), USDC_DUE);

        messageTransmitter.setFailReceive(false);
        destinationApp.completeRepayment(mandate.orderId, message, hex"a77e57");
        assertEq(_creditOutstanding(), 0);
        vm.expectRevert(CrossChainAquaApp.InvalidCctpMessage.selector);
        destinationApp.completeRepayment(mandate.orderId, message, hex"a77e57");
    }

    function test_completeRepayment_fallsBackToMakerWalletAfterStrategyIsDocked() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("docked"));

        vm.chainId(BASE_CHAIN_ID);
        address[] memory tokens = new address[](2);
        tokens[0] = address(weth);
        tokens[1] = address(baseUsdc);
        vm.prank(m2);
        baseAqua.dock(address(destinationApp), m2StrategyHash, tokens);
        destinationApp.completeRepayment(
            mandate.orderId, _message(mandate.orderId, 0, address(destinationApp)), hex"a77e57"
        );

        assertEq(baseUsdc.balanceOf(m2), USDC_DUE);
        (uint248 m2UsdcCredit, uint8 m2TokenCount) =
            baseAqua.rawBalances(m2, address(destinationApp), m2StrategyHash, address(baseUsdc));
        assertEq(m2UsdcCredit, 0);
        assertEq(m2TokenCount, type(uint8).max);
        assertEq(baseUsdc.balanceOf(address(destinationApp)), 0);
    }

    function test_fillCredit_rejectsExpiredQuoteWithoutUsingCapacityOrInventory() public {
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, MakerCreditQuote memory quote,) =
            _scenario(USDC_DUE);
        quote.expires = uint48(block.timestamp - 1);

        vm.chainId(BASE_CHAIN_ID);
        bytes memory signature = _sign(M2_PK, destinationApp.creditQuoteDigest(quote));
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.QuoteExpired.selector, quote.expires));
        destinationApp.fillCredit(order, mandate, quote, signature);
        assertEq(_creditOutstanding(), 0);
        assertEq(user.balance, 0);
    }

    function test_fillCredit_enforcesM2Capacity() public {
        vm.prank(m2);
        destinationApp.configureCreditStrategy(m2StrategyHash, address(weth), USDC_DUE - 1, true);
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, MakerCreditQuote memory quote,) =
            _scenario(USDC_DUE);

        vm.chainId(BASE_CHAIN_ID);
        bytes memory signature = _sign(M2_PK, destinationApp.creditQuoteDigest(quote));
        vm.expectRevert(
            abi.encodeWithSelector(CrossChainAquaApp.CapacityExceeded.selector, uint256(0), USDC_DUE, USDC_DUE - 1)
        );
        destinationApp.fillCredit(order, mandate, quote, signature);
        assertEq(_creditOutstanding(), 0);
    }

    function test_fillCredit_rejectsCreditQuoteNonceReplayAcrossOrders() public {
        (SolventCrossChainOrder memory order, SolventMandate memory mandate, MakerCreditQuote memory quote,) =
            _scenario(USDC_DUE);
        _fillCredit(order, mandate, quote);

        order.nonce += 1;
        mandate.orderId = destinationApp.orderIdFor(order);
        quote.orderId = mandate.orderId;
        vm.chainId(BASE_CHAIN_ID);
        bytes memory signature = _sign(M2_PK, destinationApp.creditQuoteDigest(quote));
        vm.expectRevert(abi.encodeWithSelector(CrossChainAquaApp.QuoteNonceAlreadyUsed.selector, m2, quote.nonce));
        destinationApp.fillCredit(order, mandate, quote, signature);
        assertEq(_creditOutstanding(), USDC_DUE);
    }

    function test_completeRepayment_rejectsUnexpectedMintDeltaAndRemainsRetryable() public {
        (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
        ) = _scenario(USDC_DUE);
        _fillAndSettle(order, mandate, quote, claim, keccak256("mint-delta"));
        bytes memory message = _message(mandate.orderId, 1 ether, address(destinationApp));

        vm.chainId(BASE_CHAIN_ID);
        messageTransmitter.setMintBonus(1);
        vm.expectRevert(
            abi.encodeWithSelector(
                CrossChainAquaApp.UnexpectedUsdcDelta.selector,
                USDC_DUE + MAX_CCTP_FEE - 1 ether,
                USDC_DUE + MAX_CCTP_FEE - 1 ether + 1
            )
        );
        destinationApp.completeRepayment(mandate.orderId, message, hex"a77e57");
        assertEq(baseUsdc.balanceOf(address(destinationApp)), 0);
        assertEq(_creditOutstanding(), USDC_DUE);

        messageTransmitter.setMintBonus(0);
        destinationApp.completeRepayment(mandate.orderId, message, hex"a77e57");
        assertEq(_creditOutstanding(), 0);
    }

    function _configureM2() internal {
        vm.deal(m2, 10 ether);
        vm.prank(m2);
        weth.deposit{ value: 5 ether }();
        vm.prank(m2);
        weth.approve(address(baseAqua), type(uint256).max);
        vm.prank(m2);
        baseUsdc.approve(address(baseAqua), type(uint256).max);

        address[] memory tokens = new address[](2);
        tokens[0] = address(weth);
        tokens[1] = address(baseUsdc);
        uint256[] memory amounts = new uint256[](2);
        amounts[0] = 5 ether;
        vm.prank(m2);
        m2StrategyHash = baseAqua.ship(address(destinationApp), abi.encode("m2-credit"), tokens, amounts);
        vm.prank(m2);
        destinationApp.configureCreditStrategy(m2StrategyHash, address(weth), CREDIT_CAP, true);
    }

    function _depositCompact() internal returns (uint256 id) {
        vm.chainId(ARBITRUM_CHAIN_ID);
        uint96 allocatorId = compact.__registerAllocator(address(allocator), "");
        bytes12 lockTag = bytes12(
            bytes32(
                (uint256(Scope.Multichain) << 255) | (uint256(ResetPeriod.TenMinutes) << 252)
                    | (uint256(allocatorId) << 160)
            )
        );
        tokenA.mint(user, INPUT_AMOUNT);
        vm.prank(user);
        tokenA.approve(address(compact), INPUT_AMOUNT);
        vm.prank(user);
        id = compact.depositERC20(address(tokenA), lockTag, INPUT_AMOUNT, user);
    }

    function _scenario(uint256 usdcDue)
        internal
        returns (
            SolventCrossChainOrder memory order,
            SolventMandate memory mandate,
            MakerCreditQuote memory quote,
            Claim memory claim
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
            inputToken: address(tokenA),
            inputAmount: INPUT_AMOUNT,
            destinationChainId: BASE_CHAIN_ID,
            outputToken: address(0),
            minimumOutputAmount: OUTPUT_AMOUNT,
            recipient: user,
            destinationSettler: address(destinationApp),
            fillProofVerifier: address(fillVerifier),
            exclusiveFiller: address(0),
            exclusivityEnds: 0,
            fillDeadline: uint48(block.timestamp + 1 days),
            routeKind: RouteKind.TwoMakerCredit
        });
        bytes32 orderId = destinationApp.orderIdFor(order);
        mandate = SolventMandate({
            orderId: orderId,
            destinationChainId: BASE_CHAIN_ID,
            destinationSettler: address(destinationApp),
            fillProofVerifier: address(fillVerifier),
            outputToken: address(0),
            minimumOutputAmount: OUTPUT_AMOUNT,
            recipient: user,
            fillDeadline: order.fillDeadline,
            exclusiveFiller: address(0),
            routeKind: RouteKind.TwoMakerCredit
        });
        quote = MakerCreditQuote({
            orderId: orderId,
            maker: m2,
            destinationStrategyHash: m2StrategyHash,
            outputAmount: OUTPUT_AMOUNT,
            usdcDue: usdcDue,
            maxCctpFee: MAX_CCTP_FEE,
            nonce: 11,
            expires: uint48(block.timestamp + 1 hours)
        });
        claim = _registerClaim(order, mandate);
    }

    function _registerClaim(
        SolventCrossChainOrder memory order,
        SolventMandate memory mandate
    )
        internal
        returns (Claim memory claim)
    {
        vm.chainId(ARBITRUM_CHAIN_ID);
        Component[] memory claimants = new Component[](1);
        claimants[0] = Component({ claimant: uint256(uint160(address(originSettler))), amount: INPUT_AMOUNT });
        claim = Claim({
            allocatorData: "",
            sponsorSignature: "",
            sponsor: user,
            nonce: 91,
            expires: order.compactExpires,
            witness: originSettler.mandateHash(mandate),
            witnessTypestring: originSettler.MANDATE_WITNESS_TYPESTRING(),
            id: compactId,
            allocatedAmount: INPUT_AMOUNT,
            claimants: claimants
        });
        bytes32 claimHash = keccak256(
            abi.encode(
                originSettler.COMPACT_WITH_MANDATE_TYPEHASH(),
                address(originSettler),
                user,
                claim.nonce,
                claim.expires,
                bytes12(bytes32(compactId)),
                address(tokenA),
                INPUT_AMOUNT,
                claim.witness
            )
        );
        bytes32 compactTypehash = originSettler.COMPACT_WITH_MANDATE_TYPEHASH();
        vm.prank(user);
        compact.register(claimHash, compactTypehash);
    }

    function _verifiedFill(
        SolventCrossChainOrder memory order,
        MakerCreditQuote memory quote,
        bytes32 fillId
    )
        internal
        view
        returns (IFillProofVerifier.VerifiedFill memory)
    {
        return IFillProofVerifier.VerifiedFill({
            orderId: quote.orderId,
            routeKind: RouteKind.TwoMakerCredit,
            destinationChainId: BASE_CHAIN_ID,
            destinationApp: address(destinationApp),
            recipient: order.recipient,
            outputToken: order.outputToken,
            outputAmount: quote.outputAmount,
            destinationMaker: quote.maker,
            destinationStrategyHash: quote.destinationStrategyHash,
            originStrategyHash: bytes32(0),
            repaymentToken: address(baseUsdc),
            repaymentAmount: quote.usdcDue,
            maxCctpFee: quote.maxCctpFee,
            makerQuoteHash: destinationApp.creditQuoteDigest(quote),
            fillId: fillId
        });
    }

    function _fillCredit(
        SolventCrossChainOrder memory order,
        SolventMandate memory mandate,
        MakerCreditQuote memory quote
    )
        internal
    {
        vm.chainId(BASE_CHAIN_ID);
        destinationApp.fillCredit(order, mandate, quote, _sign(M2_PK, destinationApp.creditQuoteDigest(quote)));
    }

    function _fillAndSettle(
        SolventCrossChainOrder memory order,
        SolventMandate memory mandate,
        MakerCreditQuote memory quote,
        Claim memory claim,
        bytes32 fillId
    )
        internal
    {
        _fillCredit(order, mandate, quote);
        vm.chainId(ARBITRUM_CHAIN_ID);
        originSettler.settleRouted(order, mandate, abi.encode(_verifiedFill(order, quote, fillId)), claim, makerOrder);
    }

    function _message(
        bytes32 orderId,
        uint256 feeExecuted,
        address destinationCaller
    )
        internal
        view
        returns (bytes memory)
    {
        bytes memory header = abi.encodePacked(
            uint32(1),
            ARBITRUM_CCTP_DOMAIN,
            BASE_CCTP_DOMAIN,
            keccak256(abi.encode(orderId)),
            CctpMessageLib.addressToBytes32(address(tokenMessenger)),
            CctpMessageLib.addressToBytes32(destinationTokenMessenger),
            CctpMessageLib.addressToBytes32(destinationCaller),
            CCTP_FINALITY,
            CCTP_FINALITY
        );
        bytes memory body = abi.encodePacked(
            uint32(1),
            CctpMessageLib.addressToBytes32(address(tokenB)),
            CctpMessageLib.addressToBytes32(address(destinationApp)),
            USDC_DUE + MAX_CCTP_FEE,
            CctpMessageLib.addressToBytes32(address(originSettler)),
            MAX_CCTP_FEE,
            feeExecuted,
            uint256(0),
            uint256(1),
            orderId
        );
        return bytes.concat(header, body);
    }

    function _creditOutstanding() internal view returns (uint256 outstanding) {
        (,, outstanding,) = destinationApp.creditStrategies(m2, m2StrategyHash);
    }

    function _sign(uint256 privateKey, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(privateKey, digest);
        return bytes.concat(r, s, bytes1(v));
    }
}
