// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { SignatureChecker } from "@openzeppelin/contracts/utils/cryptography/SignatureChecker.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { AquaApp } from "@1inch/aqua/src/AquaApp.sol";

import {
    CrossChainHashLib,
    DirectMakerQuote,
    MakerCreditQuote,
    RouteKind,
    SolventCrossChainOrder,
    SolventMandate
} from "./crosschain/CrossChainTypes.sol";
import { CctpMessageLib } from "./crosschain/CctpMessageLib.sol";
import { ProofKind } from "./crosschain/ProofTypes.sol";
import { IMessageTransmitterV2 } from "./interfaces/ICctpV2.sol";
import { IFillProofVerifier } from "./interfaces/IFillProofVerifier.sol";
import { IProofOutbox } from "./interfaces/IProofOutbox.sol";
import { IRepaymentProofVerifier } from "./interfaces/IRepaymentProofVerifier.sol";
import { IWETH } from "./interfaces/IWETH.sol";

/// @title CrossChainAquaApp
/// @notice Delivers a destination asset from a maker's Aqua position and tracks its repayment exposure.
contract CrossChainAquaApp is AquaApp, EIP712, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    enum ReceivableState {
        None,
        Delivered,
        Repaid
    }

    struct DirectCreditStrategy {
        address outputToken;
        uint256 maxOutstandingRepayment;
        uint256 outstandingRepayment;
        bool enabled;
    }

    struct DirectReceivable {
        address maker;
        bytes32 destinationStrategyHash;
        bytes32 originStrategyHash;
        uint256 repaymentAmount;
        uint48 filledAt;
        ReceivableState state;
    }

    struct CctpConfig {
        IERC20 usdc;
        IMessageTransmitterV2 messageTransmitter;
        uint32 originDomain;
        uint32 destinationDomain;
        bytes32 originTokenMessenger;
        address destinationTokenMessenger;
        address originUsdc;
        uint32 messageVersion;
        uint32 burnMessageVersion;
        uint32 minFinalityThreshold;
        address feeRecipient;
    }

    struct CreditStrategy {
        address outputToken;
        uint256 maxOutstandingUsdc;
        uint256 outstandingUsdc;
        bool enabled;
    }

    struct CreditReceivable {
        address maker;
        bytes32 destinationStrategyHash;
        uint256 usdcDue;
        uint256 maxCctpFee;
        uint48 filledAt;
        ReceivableState state;
    }

    uint8 private constant _DOCKED = type(uint8).max;

    IWETH public immutable WETH;
    uint256 public immutable ORIGIN_CHAIN_ID;
    address public immutable ORIGIN_SETTLER;
    address public immutable ORIGIN_COMPACT;
    address public immutable ORIGIN_TOKEN;
    address public immutable FILL_PROOF_VERIFIER;
    IProofOutbox public immutable PROOF_OUTBOX;
    IRepaymentProofVerifier public immutable REPAYMENT_PROOF_VERIFIER;
    IERC20 private immutable USDC;
    IMessageTransmitterV2 private immutable MESSAGE_TRANSMITTER;
    uint32 private immutable ORIGIN_CCTP_DOMAIN;
    uint32 private immutable DESTINATION_CCTP_DOMAIN;
    bytes32 private immutable ORIGIN_TOKEN_MESSENGER;
    address private immutable DESTINATION_TOKEN_MESSENGER;
    address private immutable ORIGIN_USDC;
    uint32 private immutable CCTP_MESSAGE_VERSION;
    uint32 private immutable CCTP_BURN_MESSAGE_VERSION;
    uint32 private immutable CCTP_MIN_FINALITY_THRESHOLD;
    address private immutable FEE_RECIPIENT;

    mapping(address maker => mapping(bytes32 strategyHash => DirectCreditStrategy)) public directStrategies;
    mapping(bytes32 orderId => DirectReceivable) public directReceivables;
    mapping(bytes32 orderId => bool) public usedOrders;
    mapping(address maker => mapping(uint256 nonce => bool)) public usedDirectQuoteNonces;
    mapping(bytes32 repaymentId => bool) public usedRepaymentProofs;
    mapping(address maker => mapping(bytes32 strategyHash => CreditStrategy)) public creditStrategies;
    mapping(bytes32 orderId => CreditReceivable) public creditReceivables;
    mapping(address maker => mapping(uint256 nonce => bool)) public usedCreditQuoteNonces;

    event DirectStrategyConfigured(
        address indexed maker,
        bytes32 indexed strategyHash,
        address indexed outputToken,
        uint256 maxOutstandingRepayment,
        bool enabled
    );
    event DirectOrderFilled(
        bytes32 indexed orderId,
        address indexed recipient,
        address outputToken,
        uint256 outputAmount,
        address indexed maker,
        bytes32 destinationStrategyHash,
        bytes32 originStrategyHash,
        uint256 repaymentAmount,
        bytes32 makerQuoteHash
    );
    event DirectReceivableClosed(bytes32 indexed orderId, address indexed maker, uint256 repaymentAmount);
    event CreditStrategyConfigured(
        address indexed maker,
        bytes32 indexed strategyHash,
        address indexed outputToken,
        uint256 maxOutstandingUsdc,
        bool enabled
    );
    event CreditOrderFilled(
        bytes32 indexed orderId,
        address indexed recipient,
        address outputToken,
        uint256 outputAmount,
        address indexed maker,
        bytes32 destinationStrategyHash,
        uint256 usdcDue,
        uint256 maxCctpFee,
        bytes32 makerQuoteHash
    );
    event CreditReceivableClosed(
        bytes32 indexed orderId,
        address indexed maker,
        uint256 usdcDue,
        uint256 feeExecuted,
        uint256 surplus,
        bool usedAquaPush
    );
    event CreditReceivableRepaidDirectTransfer(bytes32 indexed orderId, address indexed maker, uint256 usdcDue);

    error CapacityExceeded(uint256 outstanding, uint256 due, uint256 maximum);
    error CctpReceiveFailed();
    error EthDeliveryFailed(address recipient, uint256 amount);
    error FillDeadlinePassed(uint48 deadline);
    error InactiveStrategy(address maker, bytes32 strategyHash);
    error InvalidMakerQuote();
    error InvalidMandate();
    error InvalidOrder();
    error InvalidRepaymentProof();
    error InvalidCctpMessage();
    error OrderAlreadyUsed(bytes32 orderId);
    error QuoteExpired(uint48 expires);
    error QuoteNonceAlreadyUsed(address maker, uint256 nonce);
    error RepaymentProofAlreadyUsed(bytes32 repaymentId);
    error StrategyBelowOutstanding(uint256 maximum, uint256 outstanding);
    error StrategyDisabled(address maker, bytes32 strategyHash);
    error StrategyTokenMismatch(address expected, address actual);
    error UnauthorizedFiller(address caller, address exclusiveFiller);
    error UnexpectedEthSender(address sender);
    error UnexpectedOutputDelta(address token, uint256 expected, uint256 actual);
    error UnexpectedWethDelta(uint256 expected, uint256 actual);
    error UnexpectedUsdcDelta(uint256 expected, uint256 actual);
    error ZeroAddress();

    constructor(
        IAqua aqua,
        IWETH weth,
        uint256 originChainId,
        address originSettler,
        address originCompact,
        address originToken,
        address fillProofVerifier,
        IProofOutbox proofOutbox,
        IRepaymentProofVerifier repaymentProofVerifier,
        CctpConfig memory cctp
    )
        AquaApp(aqua)
        EIP712("Solvent Cross-Chain Aqua App", "1")
    {
        if (
            address(aqua) == address(0) || address(weth) == address(0) || originSettler == address(0)
                || originCompact == address(0) || originToken == address(0) || fillProofVerifier == address(0)
                || address(proofOutbox) == address(0) || address(repaymentProofVerifier) == address(0)
                || address(cctp.usdc) == address(0) || address(cctp.messageTransmitter) == address(0)
                || cctp.originTokenMessenger == bytes32(0) || cctp.destinationTokenMessenger == address(0)
                || cctp.originUsdc == address(0) || cctp.feeRecipient == address(0)
        ) {
            revert ZeroAddress();
        }
        WETH = weth;
        ORIGIN_CHAIN_ID = originChainId;
        ORIGIN_SETTLER = originSettler;
        ORIGIN_COMPACT = originCompact;
        ORIGIN_TOKEN = originToken;
        FILL_PROOF_VERIFIER = fillProofVerifier;
        PROOF_OUTBOX = proofOutbox;
        REPAYMENT_PROOF_VERIFIER = repaymentProofVerifier;
        USDC = cctp.usdc;
        MESSAGE_TRANSMITTER = cctp.messageTransmitter;
        ORIGIN_CCTP_DOMAIN = cctp.originDomain;
        DESTINATION_CCTP_DOMAIN = cctp.destinationDomain;
        ORIGIN_TOKEN_MESSENGER = cctp.originTokenMessenger;
        DESTINATION_TOKEN_MESSENGER = cctp.destinationTokenMessenger;
        ORIGIN_USDC = cctp.originUsdc;
        CCTP_MESSAGE_VERSION = cctp.messageVersion;
        CCTP_BURN_MESSAGE_VERSION = cctp.burnMessageVersion;
        CCTP_MIN_FINALITY_THRESHOLD = cctp.minFinalityThreshold;
        FEE_RECIPIENT = cctp.feeRecipient;
    }

    receive() external payable {
        require(msg.sender == address(WETH), UnexpectedEthSender(msg.sender));
    }

    /// @notice Configure one destination strategy and its bounded origin-token exposure.
    function configureDirectStrategy(
        bytes32 strategyHash,
        address outputToken,
        uint256 maxOutstandingRepayment,
        bool enabled
    )
        external
    {
        require(outputToken != address(0), ZeroAddress());
        DirectCreditStrategy storage strategy = directStrategies[msg.sender][strategyHash];
        if (strategy.outputToken != address(0)) {
            require(strategy.outputToken == outputToken, StrategyTokenMismatch(strategy.outputToken, outputToken));
        }
        require(
            maxOutstandingRepayment >= strategy.outstandingRepayment,
            StrategyBelowOutstanding(maxOutstandingRepayment, strategy.outstandingRepayment)
        );
        if (enabled) {
            (, uint8 tokensCount) = AQUA.rawBalances(msg.sender, address(this), strategyHash, outputToken);
            require(tokensCount != 0 && tokensCount != _DOCKED, InactiveStrategy(msg.sender, strategyHash));
        }
        strategy.outputToken = outputToken;
        strategy.maxOutstandingRepayment = maxOutstandingRepayment;
        strategy.enabled = enabled;
        emit DirectStrategyConfigured(msg.sender, strategyHash, outputToken, maxOutstandingRepayment, enabled);
    }

    /// @notice Configure one destination strategy and its bounded CCTP USDC exposure.
    function configureCreditStrategy(
        bytes32 strategyHash,
        address outputToken,
        uint256 maxOutstandingUsdc,
        bool enabled
    )
        external
    {
        require(outputToken != address(0), ZeroAddress());
        CreditStrategy storage strategy = creditStrategies[msg.sender][strategyHash];
        if (strategy.outputToken != address(0)) {
            require(strategy.outputToken == outputToken, StrategyTokenMismatch(strategy.outputToken, outputToken));
        }
        require(
            maxOutstandingUsdc >= strategy.outstandingUsdc,
            StrategyBelowOutstanding(maxOutstandingUsdc, strategy.outstandingUsdc)
        );
        if (enabled) {
            _requireActiveToken(msg.sender, strategyHash, outputToken);
            _requireActiveToken(msg.sender, strategyHash, address(USDC));
        }
        strategy.outputToken = outputToken;
        strategy.maxOutstandingUsdc = maxOutstandingUsdc;
        strategy.enabled = enabled;
        emit CreditStrategyConfigured(msg.sender, strategyHash, outputToken, maxOutstandingUsdc, enabled);
    }

    /// @notice Deliver the destination asset and create M1's bounded origin-token receivable.
    function fillDirect(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        DirectMakerQuote calldata quote,
        bytes calldata makerSignature
    )
        external
        nonReentrant
    {
        bytes32 orderId = CrossChainHashLib.hashOrder(order);
        _validateOrder(order, RouteKind.DirectMaker);
        _validateMandate(order, mandate, orderId);
        _validateQuote(order, quote, orderId, makerSignature);

        DirectCreditStrategy storage strategy = directStrategies[quote.maker][quote.destinationStrategyHash];
        require(strategy.enabled, StrategyDisabled(quote.maker, quote.destinationStrategyHash));
        address strategyToken = order.outputToken == address(0) ? address(WETH) : order.outputToken;
        require(strategy.outputToken == strategyToken, StrategyTokenMismatch(strategy.outputToken, strategyToken));
        require(
            strategy.outstandingRepayment + quote.repaymentAmount <= strategy.maxOutstandingRepayment,
            CapacityExceeded(strategy.outstandingRepayment, quote.repaymentAmount, strategy.maxOutstandingRepayment)
        );

        require(!usedOrders[orderId], OrderAlreadyUsed(orderId));
        require(!usedDirectQuoteNonces[quote.maker][quote.nonce], QuoteNonceAlreadyUsed(quote.maker, quote.nonce));
        usedOrders[orderId] = true;
        usedDirectQuoteNonces[quote.maker][quote.nonce] = true;
        strategy.outstandingRepayment += quote.repaymentAmount;
        directReceivables[orderId] = DirectReceivable({
            maker: quote.maker,
            destinationStrategyHash: quote.destinationStrategyHash,
            originStrategyHash: quote.originStrategyHash,
            repaymentAmount: quote.repaymentAmount,
            filledAt: uint48(block.timestamp),
            state: ReceivableState.Delivered
        });

        if (order.outputToken == address(0)) {
            _deliverNative(order.recipient, quote.maker, quote.destinationStrategyHash, quote.outputAmount);
        } else {
            _deliverErc20(
                order.outputToken, order.recipient, quote.maker, quote.destinationStrategyHash, quote.outputAmount
            );
        }

        bytes32 makerQuoteHash = _hashTypedDataV4(CrossChainHashLib.hashDirectQuote(quote));
        _recordFill(
            order,
            orderId,
            RouteKind.DirectMaker,
            quote.maker,
            quote.destinationStrategyHash,
            quote.originStrategyHash,
            quote.outputAmount,
            ORIGIN_TOKEN,
            quote.repaymentAmount,
            0,
            makerQuoteHash
        );
        emit DirectOrderFilled(
            orderId,
            order.recipient,
            order.outputToken,
            quote.outputAmount,
            quote.maker,
            quote.destinationStrategyHash,
            quote.originStrategyHash,
            quote.repaymentAmount,
            makerQuoteHash
        );
    }

    /// @notice Deliver the destination asset and create M2's bounded USDC receivable.
    function fillCredit(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        MakerCreditQuote calldata quote,
        bytes calldata makerSignature
    )
        external
        nonReentrant
    {
        bytes32 orderId = CrossChainHashLib.hashOrder(order);
        _validateOrder(order, RouteKind.TwoMakerCredit);
        _validateMandate(order, mandate, orderId);
        _validateCreditQuote(order, quote, orderId, makerSignature);

        CreditStrategy storage strategy = creditStrategies[quote.maker][quote.destinationStrategyHash];
        require(strategy.enabled, StrategyDisabled(quote.maker, quote.destinationStrategyHash));
        address strategyToken = order.outputToken == address(0) ? address(WETH) : order.outputToken;
        require(strategy.outputToken == strategyToken, StrategyTokenMismatch(strategy.outputToken, strategyToken));
        require(
            strategy.outstandingUsdc + quote.usdcDue <= strategy.maxOutstandingUsdc,
            CapacityExceeded(strategy.outstandingUsdc, quote.usdcDue, strategy.maxOutstandingUsdc)
        );

        require(!usedOrders[orderId], OrderAlreadyUsed(orderId));
        require(!usedCreditQuoteNonces[quote.maker][quote.nonce], QuoteNonceAlreadyUsed(quote.maker, quote.nonce));
        usedOrders[orderId] = true;
        usedCreditQuoteNonces[quote.maker][quote.nonce] = true;
        strategy.outstandingUsdc += quote.usdcDue;
        creditReceivables[orderId] = CreditReceivable({
            maker: quote.maker,
            destinationStrategyHash: quote.destinationStrategyHash,
            usdcDue: quote.usdcDue,
            maxCctpFee: quote.maxCctpFee,
            filledAt: uint48(block.timestamp),
            state: ReceivableState.Delivered
        });

        if (order.outputToken == address(0)) {
            _deliverNative(order.recipient, quote.maker, quote.destinationStrategyHash, quote.outputAmount);
        } else {
            _deliverErc20(
                order.outputToken, order.recipient, quote.maker, quote.destinationStrategyHash, quote.outputAmount
            );
        }

        bytes32 makerQuoteHash = _hashTypedDataV4(CrossChainHashLib.hashCreditQuote(quote));
        _recordFill(
            order,
            orderId,
            RouteKind.TwoMakerCredit,
            quote.maker,
            quote.destinationStrategyHash,
            bytes32(0),
            quote.outputAmount,
            address(USDC),
            quote.usdcDue,
            quote.maxCctpFee,
            makerQuoteHash
        );
        emit CreditOrderFilled(
            orderId,
            order.recipient,
            order.outputToken,
            quote.outputAmount,
            quote.maker,
            quote.destinationStrategyHash,
            quote.usdcDue,
            quote.maxCctpFee,
            makerQuoteHash
        );
    }

    /// @notice Mint the CCTP transfer and atomically repay M2's Aqua strategy.
    function completeRepayment(
        bytes32 orderId,
        bytes calldata message,
        bytes calldata attestation
    )
        external
        nonReentrant
    {
        CreditReceivable storage receivable = creditReceivables[orderId];
        require(receivable.state == ReceivableState.Delivered, InvalidCctpMessage());

        CctpMessageLib.ParsedMessage memory parsed = CctpMessageLib.parse(message);
        _validateCctpMessage(orderId, receivable, parsed);

        uint256 balanceBefore = USDC.balanceOf(address(this));
        receivable.state = ReceivableState.Repaid;
        creditStrategies[receivable.maker][receivable.destinationStrategyHash].outstandingUsdc -= receivable.usdcDue;
        require(MESSAGE_TRANSMITTER.receiveMessage(message, attestation), CctpReceiveFailed());

        uint256 expectedMint = parsed.amount - parsed.feeExecuted;
        uint256 balanceAfterMint = USDC.balanceOf(address(this));
        uint256 minted = balanceAfterMint >= balanceBefore ? balanceAfterMint - balanceBefore : 0;
        require(minted == expectedMint, UnexpectedUsdcDelta(expectedMint, minted));

        (, uint8 tokensCount) =
            AQUA.rawBalances(receivable.maker, address(this), receivable.destinationStrategyHash, address(USDC));
        bool usedAquaPush = tokensCount != 0 && tokensCount != _DOCKED;
        if (usedAquaPush) {
            USDC.forceApprove(address(AQUA), receivable.usdcDue);
            AQUA.push(
                receivable.maker, address(this), receivable.destinationStrategyHash, address(USDC), receivable.usdcDue
            );
            USDC.forceApprove(address(AQUA), 0);
        } else {
            USDC.safeTransfer(receivable.maker, receivable.usdcDue);
            emit CreditReceivableRepaidDirectTransfer(orderId, receivable.maker, receivable.usdcDue);
        }

        uint256 surplus = expectedMint - receivable.usdcDue;
        if (surplus != 0) {
            USDC.safeTransfer(FEE_RECIPIENT, surplus);
        }
        require(
            USDC.balanceOf(address(this)) == balanceBefore,
            UnexpectedUsdcDelta(balanceBefore, USDC.balanceOf(address(this)))
        );
        emit CreditReceivableClosed(
            orderId, receivable.maker, receivable.usdcDue, parsed.feeExecuted, surplus, usedAquaPush
        );
    }

    /// @notice Close M1's capacity after authenticating its Arbitrum repayment event.
    function confirmDirectRepayment(bytes32 orderId, bytes calldata repaymentProof) external nonReentrant {
        IRepaymentProofVerifier.VerifiedRepayment memory repayment =
            REPAYMENT_PROOF_VERIFIER.verifyRepayment(repaymentProof);
        require(!usedRepaymentProofs[repayment.repaymentId], RepaymentProofAlreadyUsed(repayment.repaymentId));

        DirectReceivable storage receivable = directReceivables[orderId];
        require(
            receivable.state == ReceivableState.Delivered && repayment.orderId == orderId
                && repayment.originChainId == ORIGIN_CHAIN_ID && repayment.originSettler == ORIGIN_SETTLER
                && repayment.maker == receivable.maker && repayment.repaymentToken == ORIGIN_TOKEN
                && repayment.repaymentAmount == receivable.repaymentAmount && repayment.repaymentId != bytes32(0),
            InvalidRepaymentProof()
        );

        usedRepaymentProofs[repayment.repaymentId] = true;
        receivable.state = ReceivableState.Repaid;
        directStrategies[receivable.maker][receivable.destinationStrategyHash].outstandingRepayment -= receivable.repaymentAmount;
        emit DirectReceivableClosed(orderId, receivable.maker, receivable.repaymentAmount);
    }

    function orderIdFor(SolventCrossChainOrder calldata order) external pure returns (bytes32) {
        return CrossChainHashLib.hashOrder(order);
    }

    function mandateHash(SolventMandate calldata mandate) external pure returns (bytes32) {
        return CrossChainHashLib.hashMandate(mandate);
    }

    function directQuoteDigest(DirectMakerQuote calldata quote) external view returns (bytes32) {
        return _hashTypedDataV4(CrossChainHashLib.hashDirectQuote(quote));
    }

    function creditQuoteDigest(MakerCreditQuote calldata quote) external view returns (bytes32) {
        return _hashTypedDataV4(CrossChainHashLib.hashCreditQuote(quote));
    }

    function _recordFill(
        SolventCrossChainOrder calldata order,
        bytes32 orderId,
        RouteKind routeKind,
        address maker,
        bytes32 destinationStrategyHash,
        bytes32 originStrategyHash,
        uint256 outputAmount,
        address repaymentToken,
        uint256 repaymentAmount,
        uint256 maxCctpFee,
        bytes32 makerQuoteHash
    )
        private
    {
        bytes32 fillId = keccak256(abi.encode(block.chainid, address(this), orderId, ProofKind.Fill));
        IFillProofVerifier.VerifiedFill memory fill = IFillProofVerifier.VerifiedFill({
            orderId: orderId,
            routeKind: routeKind,
            destinationChainId: block.chainid,
            destinationApp: address(this),
            recipient: order.recipient,
            outputToken: order.outputToken,
            outputAmount: outputAmount,
            destinationMaker: maker,
            destinationStrategyHash: destinationStrategyHash,
            originStrategyHash: originStrategyHash,
            repaymentToken: repaymentToken,
            repaymentAmount: repaymentAmount,
            maxCctpFee: maxCctpFee,
            makerQuoteHash: makerQuoteHash,
            fillId: fillId
        });
        PROOF_OUTBOX.record(orderId, ProofKind.Fill, abi.encode(fill));
    }

    function _validateOrder(SolventCrossChainOrder calldata order, RouteKind expectedRoute) private view {
        require(
            order.user != address(0) && order.recipient != address(0) && order.recipient != address(this)
                && order.originChainId == ORIGIN_CHAIN_ID && order.originSettler == ORIGIN_SETTLER
                && order.compact == ORIGIN_COMPACT && order.inputToken == ORIGIN_TOKEN
                && address(uint160(order.compactId)) == ORIGIN_TOKEN && order.destinationChainId == block.chainid
                && order.destinationSettler == address(this) && order.fillProofVerifier == FILL_PROOF_VERIFIER
                && order.routeKind == expectedRoute && order.inputAmount != 0 && order.minimumOutputAmount != 0
                && order.fillDeadline < order.compactExpires,
            InvalidOrder()
        );
        require(block.timestamp <= order.fillDeadline, FillDeadlinePassed(order.fillDeadline));
        if (order.exclusiveFiller != address(0) && block.timestamp < order.exclusivityEnds) {
            require(msg.sender == order.exclusiveFiller, UnauthorizedFiller(msg.sender, order.exclusiveFiller));
        }
    }

    function _validateMandate(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        bytes32 orderId
    )
        private
        pure
    {
        require(
            mandate.orderId == orderId && mandate.destinationChainId == order.destinationChainId
                && mandate.destinationSettler == order.destinationSettler
                && mandate.fillProofVerifier == order.fillProofVerifier && mandate.outputToken == order.outputToken
                && mandate.minimumOutputAmount == order.minimumOutputAmount && mandate.recipient == order.recipient
                && mandate.fillDeadline == order.fillDeadline && mandate.exclusiveFiller == order.exclusiveFiller
                && mandate.routeKind == order.routeKind,
            InvalidMandate()
        );
    }

    function _validateQuote(
        SolventCrossChainOrder calldata order,
        DirectMakerQuote calldata quote,
        bytes32 orderId,
        bytes calldata makerSignature
    )
        private
        view
    {
        require(block.timestamp <= quote.expires, QuoteExpired(quote.expires));
        bytes32 digest = _hashTypedDataV4(CrossChainHashLib.hashDirectQuote(quote));
        require(
            quote.orderId == orderId && quote.maker != address(0) && quote.originStrategyHash != bytes32(0)
                && quote.outputAmount >= order.minimumOutputAmount && quote.repaymentAmount == order.inputAmount
                && SignatureChecker.isValidSignatureNow(quote.maker, digest, makerSignature),
            InvalidMakerQuote()
        );
    }

    function _validateCreditQuote(
        SolventCrossChainOrder calldata order,
        MakerCreditQuote calldata quote,
        bytes32 orderId,
        bytes calldata makerSignature
    )
        private
        view
    {
        require(block.timestamp <= quote.expires, QuoteExpired(quote.expires));
        bytes32 digest = _hashTypedDataV4(CrossChainHashLib.hashCreditQuote(quote));
        require(
            quote.orderId == orderId && quote.maker != address(0) && quote.outputAmount >= order.minimumOutputAmount
                && quote.usdcDue != 0 && SignatureChecker.isValidSignatureNow(quote.maker, digest, makerSignature),
            InvalidMakerQuote()
        );
    }

    function _validateCctpMessage(
        bytes32 orderId,
        CreditReceivable storage receivable,
        CctpMessageLib.ParsedMessage memory parsed
    )
        private
        view
    {
        uint256 burnAmount = receivable.usdcDue + receivable.maxCctpFee;
        require(
            parsed.version == CCTP_MESSAGE_VERSION && parsed.sourceDomain == ORIGIN_CCTP_DOMAIN
                && parsed.destinationDomain == DESTINATION_CCTP_DOMAIN && parsed.nonce != bytes32(0)
                && parsed.sender == ORIGIN_TOKEN_MESSENGER
                && parsed.recipient == CctpMessageLib.addressToBytes32(DESTINATION_TOKEN_MESSENGER)
                && parsed.destinationCaller == CctpMessageLib.addressToBytes32(address(this))
                && parsed.minFinalityThreshold == CCTP_MIN_FINALITY_THRESHOLD
                && parsed.finalityThresholdExecuted >= CCTP_MIN_FINALITY_THRESHOLD
                && parsed.burnVersion == CCTP_BURN_MESSAGE_VERSION
                && parsed.burnToken == CctpMessageLib.addressToBytes32(ORIGIN_USDC)
                && parsed.mintRecipient == CctpMessageLib.addressToBytes32(address(this)) && parsed.amount == burnAmount
                && parsed.messageSender == CctpMessageLib.addressToBytes32(ORIGIN_SETTLER)
                && parsed.maxFee == receivable.maxCctpFee && parsed.feeExecuted <= receivable.maxCctpFee
                && parsed.hookVersion == CctpMessageLib.HOOK_VERSION && parsed.hookOrderId == orderId
                && burnAmount - parsed.feeExecuted >= receivable.usdcDue,
            InvalidCctpMessage()
        );
    }

    function _requireActiveToken(address maker, bytes32 strategyHash, address token) private view {
        (, uint8 tokensCount) = AQUA.rawBalances(maker, address(this), strategyHash, token);
        require(tokensCount != 0 && tokensCount != _DOCKED, InactiveStrategy(maker, strategyHash));
    }

    function _deliverNative(address recipient, address maker, bytes32 strategyHash, uint256 amount) private {
        uint256 wethBefore = WETH.balanceOf(address(this));
        uint256 ethBefore = address(this).balance;
        AQUA.pull(maker, strategyHash, address(WETH), amount, address(this));
        uint256 wethAfterPull = WETH.balanceOf(address(this));
        uint256 wethDelta = wethAfterPull >= wethBefore ? wethAfterPull - wethBefore : 0;
        require(wethDelta == amount, UnexpectedWethDelta(amount, wethDelta));

        WETH.withdraw(amount);
        (bool delivered,) = payable(recipient).call{ value: amount }("");
        require(delivered, EthDeliveryFailed(recipient, amount));
        require(
            WETH.balanceOf(address(this)) == wethBefore, UnexpectedWethDelta(wethBefore, WETH.balanceOf(address(this)))
        );
        require(address(this).balance == ethBefore, EthDeliveryFailed(recipient, amount));
    }

    function _deliverErc20(
        address token,
        address recipient,
        address maker,
        bytes32 strategyHash,
        uint256 amount
    )
        private
    {
        uint256 recipientBefore = IERC20(token).balanceOf(recipient);
        AQUA.pull(maker, strategyHash, token, amount, recipient);
        uint256 recipientAfter = IERC20(token).balanceOf(recipient);
        uint256 outputDelta = recipientAfter >= recipientBefore ? recipientAfter - recipientBefore : 0;
        require(outputDelta == amount, UnexpectedOutputDelta(token, amount, outputDelta));
    }
}
