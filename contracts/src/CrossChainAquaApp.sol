// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { SignatureChecker } from "@openzeppelin/contracts/utils/cryptography/SignatureChecker.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";

import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { AquaApp } from "@1inch/aqua/src/AquaApp.sol";

import {
    CrossChainHashLib,
    DirectMakerQuote,
    RouteKind,
    SolventCrossChainOrder,
    SolventMandate
} from "./crosschain/CrossChainTypes.sol";
import { IRepaymentProofVerifier } from "./interfaces/IRepaymentProofVerifier.sol";
import { IWETH } from "./interfaces/IWETH.sol";

/// @title CrossChainAquaApp
/// @notice Delivers a destination asset from a maker's Aqua position and tracks its repayment exposure.
contract CrossChainAquaApp is AquaApp, EIP712, ReentrancyGuardTransient {
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

    uint8 private constant _DOCKED = type(uint8).max;

    IWETH public immutable WETH;
    uint256 public immutable ORIGIN_CHAIN_ID;
    address public immutable ORIGIN_SETTLER;
    address public immutable ORIGIN_COMPACT;
    address public immutable ORIGIN_TOKEN;
    address public immutable FILL_PROOF_VERIFIER;
    IRepaymentProofVerifier public immutable REPAYMENT_PROOF_VERIFIER;

    mapping(address maker => mapping(bytes32 strategyHash => DirectCreditStrategy)) public directStrategies;
    mapping(bytes32 orderId => DirectReceivable) public directReceivables;
    mapping(bytes32 orderId => bool) public usedOrders;
    mapping(address maker => mapping(uint256 nonce => bool)) public usedDirectQuoteNonces;
    mapping(bytes32 repaymentId => bool) public usedRepaymentProofs;

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

    error CapacityExceeded(uint256 outstanding, uint256 due, uint256 maximum);
    error EthDeliveryFailed(address recipient, uint256 amount);
    error FillDeadlinePassed(uint48 deadline);
    error InactiveStrategy(address maker, bytes32 strategyHash);
    error InvalidMakerQuote();
    error InvalidMandate();
    error InvalidOrder();
    error InvalidRepaymentProof();
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
    error ZeroAddress();

    constructor(
        IAqua aqua,
        IWETH weth,
        uint256 originChainId,
        address originSettler,
        address originCompact,
        address originToken,
        address fillProofVerifier,
        IRepaymentProofVerifier repaymentProofVerifier
    )
        AquaApp(aqua)
        EIP712("Solvent Cross-Chain Aqua App", "1")
    {
        if (
            address(aqua) == address(0) || address(weth) == address(0) || originSettler == address(0)
                || originCompact == address(0) || originToken == address(0) || fillProofVerifier == address(0)
                || address(repaymentProofVerifier) == address(0)
        ) {
            revert ZeroAddress();
        }
        WETH = weth;
        ORIGIN_CHAIN_ID = originChainId;
        ORIGIN_SETTLER = originSettler;
        ORIGIN_COMPACT = originCompact;
        ORIGIN_TOKEN = originToken;
        FILL_PROOF_VERIFIER = fillProofVerifier;
        REPAYMENT_PROOF_VERIFIER = repaymentProofVerifier;
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
        _validateOrder(order);
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

    function _validateOrder(SolventCrossChainOrder calldata order) private view {
        require(
            order.user != address(0) && order.recipient != address(0) && order.recipient != address(this)
                && order.originChainId == ORIGIN_CHAIN_ID && order.originSettler == ORIGIN_SETTLER
                && order.compact == ORIGIN_COMPACT && order.inputToken == ORIGIN_TOKEN
                && address(uint160(order.compactId)) == ORIGIN_TOKEN && order.destinationChainId == block.chainid
                && order.destinationSettler == address(this) && order.fillProofVerifier == FILL_PROOF_VERIFIER
                && order.routeKind == RouteKind.DirectMaker && order.inputAmount != 0 && order.minimumOutputAmount != 0
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
