// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";
import { ITheCompactClaims } from "the-compact/src/interfaces/ITheCompactClaims.sol";
import { Claim } from "the-compact/src/types/Claims.sol";

import { CrossChainHashLib, RouteKind, SolventCrossChainOrder, SolventMandate } from "./crosschain/CrossChainTypes.sol";
import { CctpMessageLib } from "./crosschain/CctpMessageLib.sol";
import { ProofKind } from "./crosschain/ProofTypes.sol";
import { ITokenMessengerV2 } from "./interfaces/ICctpV2.sol";
import { IFillProofVerifier } from "./interfaces/IFillProofVerifier.sol";
import { IProofOutbox } from "./interfaces/IProofOutbox.sol";
import { IRepaymentProofVerifier } from "./interfaces/IRepaymentProofVerifier.sol";

/// @title CompactOriginSettler
/// @notice Claims a proven cross-chain order and atomically repays its origin-side maker.
contract CompactOriginSettler is ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    uint8 private constant _DOCKED = type(uint8).max;

    struct RoutedConfig {
        IERC20 originUsdc;
        ISwapVM swapRouter;
        ITokenMessengerV2 tokenMessenger;
        address destinationUsdc;
        uint32 destinationDomain;
        uint32 minFinalityThreshold;
        address feeRecipient;
    }

    string public constant MANDATE_WITNESS_TYPESTRING =
        "bytes32 orderId,uint256 destinationChainId,address destinationSettler,address fillProofVerifier,address outputToken,uint256 minimumOutputAmount,address recipient,uint48 fillDeadline,address exclusiveFiller,uint8 routeKind";
    bytes32 public constant COMPACT_WITH_MANDATE_TYPEHASH = keccak256(
        "Compact(address arbiter,address sponsor,uint256 nonce,uint256 expires,bytes12 lockTag,address token,uint256 amount,Mandate mandate)Mandate(bytes32 orderId,uint256 destinationChainId,address destinationSettler,address fillProofVerifier,address outputToken,uint256 minimumOutputAmount,address recipient,uint48 fillDeadline,address exclusiveFiller,uint8 routeKind)"
    );

    ITheCompactClaims public immutable COMPACT;
    IAqua public immutable AQUA;
    IERC20 public immutable ORIGIN_TOKEN;
    uint256 public immutable DESTINATION_CHAIN_ID;
    address public immutable DESTINATION_APP;
    IFillProofVerifier public immutable FILL_PROOF_VERIFIER;
    IProofOutbox public immutable PROOF_OUTBOX;
    IERC20 private immutable ORIGIN_USDC;
    ISwapVM private immutable SWAP_ROUTER;
    ITokenMessengerV2 private immutable TOKEN_MESSENGER;
    address private immutable DESTINATION_USDC;
    uint32 private immutable DESTINATION_CCTP_DOMAIN;
    uint32 private immutable CCTP_MIN_FINALITY_THRESHOLD;
    address private immutable FEE_RECIPIENT;

    mapping(bytes32 orderId => bool) public settledOrders;
    mapping(bytes32 fillId => bool) public usedFillProofs;

    event DirectMakerRepaid(
        bytes32 indexed orderId,
        address indexed maker,
        address indexed repaymentToken,
        uint256 repaymentAmount,
        bool usedAquaPush
    );
    event RoutedBurnInitiated(
        bytes32 indexed orderId,
        address indexed originMaker,
        bytes32 indexed swapOrderHash,
        uint256 inputAmount,
        uint256 usdcBurned,
        uint256 maxCctpFee,
        uint256 surplus
    );

    error FillProofAlreadyUsed(bytes32 fillId);
    error InvalidCompactClaim();
    error InvalidFillProof();
    error InvalidMandate();
    error InvalidOrder();
    error InvalidSwapResult();
    error OrderAlreadySettled(bytes32 orderId);
    error UnexpectedRepaymentDelta(uint256 expected, uint256 actual);
    error UnexpectedTokenBalance(address token, uint256 expected, uint256 actual);
    error ZeroAddress();

    constructor(
        ITheCompactClaims compact,
        IAqua aqua,
        IERC20 originToken,
        uint256 destinationChainId,
        address destinationApp,
        IFillProofVerifier fillProofVerifier,
        IProofOutbox proofOutbox,
        RoutedConfig memory routed
    ) {
        if (
            address(compact) == address(0) || address(aqua) == address(0) || address(originToken) == address(0)
                || destinationApp == address(0) || address(fillProofVerifier) == address(0)
                || address(proofOutbox) == address(0) || address(routed.originUsdc) == address(0)
                || address(routed.swapRouter) == address(0) || address(routed.tokenMessenger) == address(0)
                || routed.destinationUsdc == address(0) || routed.feeRecipient == address(0)
        ) {
            revert ZeroAddress();
        }
        COMPACT = compact;
        AQUA = aqua;
        ORIGIN_TOKEN = originToken;
        DESTINATION_CHAIN_ID = destinationChainId;
        DESTINATION_APP = destinationApp;
        FILL_PROOF_VERIFIER = fillProofVerifier;
        PROOF_OUTBOX = proofOutbox;
        ORIGIN_USDC = routed.originUsdc;
        SWAP_ROUTER = routed.swapRouter;
        TOKEN_MESSENGER = routed.tokenMessenger;
        DESTINATION_USDC = routed.destinationUsdc;
        DESTINATION_CCTP_DOMAIN = routed.destinationDomain;
        CCTP_MIN_FINALITY_THRESHOLD = routed.minFinalityThreshold;
        FEE_RECIPIENT = routed.feeRecipient;
    }

    /// @notice Settle Case 1 after authenticating the destination fill.
    function settleDirect(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        bytes calldata fillProof,
        Claim calldata compactClaim
    )
        external
        nonReentrant
    {
        bytes32 orderId = CrossChainHashLib.hashOrder(order);
        _validateOrder(order, RouteKind.DirectMaker);
        _validateMandate(order, mandate, orderId);

        IFillProofVerifier.VerifiedFill memory fill = FILL_PROOF_VERIFIER.verifyFill(fillProof);
        _validateFill(order, orderId, fill);
        require(!settledOrders[orderId], OrderAlreadySettled(orderId));
        require(!usedFillProofs[fill.fillId], FillProofAlreadyUsed(fill.fillId));
        _validateClaim(order, mandate, compactClaim);

        settledOrders[orderId] = true;
        usedFillProofs[fill.fillId] = true;

        uint256 balanceBefore = ORIGIN_TOKEN.balanceOf(address(this));
        COMPACT.claim(compactClaim);
        uint256 balanceAfterClaim = ORIGIN_TOKEN.balanceOf(address(this));
        uint256 claimed = balanceAfterClaim >= balanceBefore ? balanceAfterClaim - balanceBefore : 0;
        require(claimed == fill.repaymentAmount, UnexpectedRepaymentDelta(fill.repaymentAmount, claimed));

        (, uint8 tokensCount) =
            AQUA.rawBalances(fill.destinationMaker, address(this), fill.originStrategyHash, address(ORIGIN_TOKEN));
        bool usedAquaPush = tokensCount != 0 && tokensCount != _DOCKED;
        if (usedAquaPush) {
            ORIGIN_TOKEN.forceApprove(address(AQUA), fill.repaymentAmount);
            AQUA.push(
                fill.destinationMaker,
                address(this),
                fill.originStrategyHash,
                address(ORIGIN_TOKEN),
                fill.repaymentAmount
            );
            ORIGIN_TOKEN.forceApprove(address(AQUA), 0);
        } else {
            ORIGIN_TOKEN.safeTransfer(fill.destinationMaker, fill.repaymentAmount);
        }

        require(
            ORIGIN_TOKEN.balanceOf(address(this)) == balanceBefore,
            UnexpectedRepaymentDelta(balanceBefore, ORIGIN_TOKEN.balanceOf(address(this)))
        );
        emit DirectMakerRepaid(
            orderId, fill.destinationMaker, address(ORIGIN_TOKEN), fill.repaymentAmount, usedAquaPush
        );
        _recordDirectRepayment(orderId, fill);
    }

    /// @notice Claim the source token, swap it against M1's Aqua strategy, and burn CCTP USDC atomically.
    function settleRouted(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        bytes calldata fillProof,
        Claim calldata compactClaim,
        ISwapVM.Order calldata makerOrder
    )
        external
        nonReentrant
    {
        bytes32 orderId = CrossChainHashLib.hashOrder(order);
        _validateOrder(order, RouteKind.TwoMakerCredit);
        _validateMandate(order, mandate, orderId);

        IFillProofVerifier.VerifiedFill memory fill = FILL_PROOF_VERIFIER.verifyFill(fillProof);
        _validateRoutedFill(order, orderId, fill);
        require(!settledOrders[orderId], OrderAlreadySettled(orderId));
        require(!usedFillProofs[fill.fillId], FillProofAlreadyUsed(fill.fillId));
        _validateClaim(order, mandate, compactClaim);

        settledOrders[orderId] = true;
        usedFillProofs[fill.fillId] = true;

        uint256 inputBalanceBefore = ORIGIN_TOKEN.balanceOf(address(this));
        uint256 usdcBalanceBefore = ORIGIN_USDC.balanceOf(address(this));
        COMPACT.claim(compactClaim);
        uint256 inputBalanceAfterClaim = ORIGIN_TOKEN.balanceOf(address(this));
        uint256 claimed = inputBalanceAfterClaim >= inputBalanceBefore ? inputBalanceAfterClaim - inputBalanceBefore : 0;
        require(claimed == order.inputAmount, UnexpectedRepaymentDelta(order.inputAmount, claimed));

        uint256 burnAmount = fill.repaymentAmount + fill.maxCctpFee;
        ORIGIN_TOKEN.forceApprove(address(SWAP_ROUTER), order.inputAmount);
        (uint256 amountIn, uint256 amountOut, bytes32 swapOrderHash) = SWAP_ROUTER.swap(
            makerOrder, address(ORIGIN_TOKEN), address(ORIGIN_USDC), order.inputAmount, _exactInputTraits(burnAmount)
        );
        ORIGIN_TOKEN.forceApprove(address(SWAP_ROUTER), 0);

        uint256 usdcBalanceAfterSwap = ORIGIN_USDC.balanceOf(address(this));
        uint256 received = usdcBalanceAfterSwap >= usdcBalanceBefore ? usdcBalanceAfterSwap - usdcBalanceBefore : 0;
        require(amountIn == order.inputAmount && amountOut == received && received >= burnAmount, InvalidSwapResult());

        ORIGIN_USDC.forceApprove(address(TOKEN_MESSENGER), burnAmount);
        TOKEN_MESSENGER.depositForBurnWithHook(
            burnAmount,
            DESTINATION_CCTP_DOMAIN,
            CctpMessageLib.addressToBytes32(DESTINATION_APP),
            address(ORIGIN_USDC),
            CctpMessageLib.addressToBytes32(DESTINATION_APP),
            fill.maxCctpFee,
            CCTP_MIN_FINALITY_THRESHOLD,
            CctpMessageLib.repaymentHook(orderId)
        );
        ORIGIN_USDC.forceApprove(address(TOKEN_MESSENGER), 0);

        uint256 surplus = received - burnAmount;
        if (surplus != 0) {
            ORIGIN_USDC.safeTransfer(FEE_RECIPIENT, surplus);
        }
        require(
            ORIGIN_TOKEN.balanceOf(address(this)) == inputBalanceBefore,
            UnexpectedTokenBalance(address(ORIGIN_TOKEN), inputBalanceBefore, ORIGIN_TOKEN.balanceOf(address(this)))
        );
        require(
            ORIGIN_USDC.balanceOf(address(this)) == usdcBalanceBefore,
            UnexpectedTokenBalance(address(ORIGIN_USDC), usdcBalanceBefore, ORIGIN_USDC.balanceOf(address(this)))
        );
        emit RoutedBurnInitiated(
            orderId, makerOrder.maker, swapOrderHash, order.inputAmount, burnAmount, fill.maxCctpFee, surplus
        );
    }

    function orderIdFor(SolventCrossChainOrder calldata order) external pure returns (bytes32) {
        return CrossChainHashLib.hashOrder(order);
    }

    function mandateHash(SolventMandate calldata mandate) external pure returns (bytes32) {
        return CrossChainHashLib.hashMandate(mandate);
    }

    function _recordDirectRepayment(bytes32 orderId, IFillProofVerifier.VerifiedFill memory fill) private {
        bytes32 repaymentId = keccak256(abi.encode(block.chainid, address(this), orderId, ProofKind.DirectRepayment));
        IRepaymentProofVerifier.VerifiedRepayment memory repayment = IRepaymentProofVerifier.VerifiedRepayment({
            orderId: orderId,
            originChainId: block.chainid,
            originSettler: address(this),
            maker: fill.destinationMaker,
            repaymentToken: address(ORIGIN_TOKEN),
            repaymentAmount: fill.repaymentAmount,
            repaymentId: repaymentId
        });
        PROOF_OUTBOX.record(orderId, ProofKind.DirectRepayment, abi.encode(repayment));
    }

    function _validateOrder(SolventCrossChainOrder calldata order, RouteKind expectedRoute) private view {
        require(
            order.user != address(0) && order.originChainId == block.chainid && order.originSettler == address(this)
                && order.compact == address(COMPACT) && order.inputToken == address(ORIGIN_TOKEN)
                && address(uint160(order.compactId)) == address(ORIGIN_TOKEN) && order.inputAmount != 0
                && order.compactExpires > order.fillDeadline && order.destinationChainId == DESTINATION_CHAIN_ID
                && order.destinationSettler == DESTINATION_APP
                && order.fillProofVerifier == address(FILL_PROOF_VERIFIER) && order.routeKind == expectedRoute,
            InvalidOrder()
        );
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

    function _validateFill(
        SolventCrossChainOrder calldata order,
        bytes32 orderId,
        IFillProofVerifier.VerifiedFill memory fill
    )
        private
        view
    {
        require(
            fill.orderId == orderId && fill.routeKind == RouteKind.DirectMaker
                && fill.destinationChainId == DESTINATION_CHAIN_ID && fill.destinationApp == DESTINATION_APP
                && fill.recipient == order.recipient && fill.outputToken == order.outputToken
                && fill.outputAmount >= order.minimumOutputAmount && fill.destinationMaker != address(0)
                && fill.destinationStrategyHash != bytes32(0) && fill.originStrategyHash != bytes32(0)
                && fill.repaymentToken == address(ORIGIN_TOKEN) && fill.repaymentAmount == order.inputAmount
                && fill.maxCctpFee == 0 && fill.makerQuoteHash != bytes32(0) && fill.fillId != bytes32(0),
            InvalidFillProof()
        );
    }

    function _validateRoutedFill(
        SolventCrossChainOrder calldata order,
        bytes32 orderId,
        IFillProofVerifier.VerifiedFill memory fill
    )
        private
        view
    {
        require(
            fill.orderId == orderId && fill.routeKind == RouteKind.TwoMakerCredit
                && fill.destinationChainId == DESTINATION_CHAIN_ID && fill.destinationApp == DESTINATION_APP
                && fill.recipient == order.recipient && fill.outputToken == order.outputToken
                && fill.outputAmount >= order.minimumOutputAmount && fill.destinationMaker != address(0)
                && fill.destinationStrategyHash != bytes32(0) && fill.originStrategyHash == bytes32(0)
                && fill.repaymentToken == DESTINATION_USDC && fill.repaymentAmount != 0
                && fill.makerQuoteHash != bytes32(0) && fill.fillId != bytes32(0),
            InvalidFillProof()
        );
    }

    function _exactInputTraits(uint256 minimumOutput) private view returns (bytes memory) {
        return TakerTraitsLib.build(
            TakerTraitsLib.Args({
                taker: address(this),
                isExactIn: true,
                shouldUnwrapWeth: false,
                isStrictThresholdAmount: false,
                isFirstTransferFromTaker: true,
                useTransferFromAndAquaPush: true,
                threshold: abi.encode(minimumOutput),
                to: address(this),
                deadline: 0,
                hasPreTransferInCallback: false,
                hasPreTransferOutCallback: false,
                preTransferInHookData: "",
                postTransferInHookData: "",
                preTransferOutHookData: "",
                postTransferOutHookData: "",
                preTransferInCallbackData: "",
                preTransferOutCallbackData: "",
                instructionsArgs: "",
                signature: ""
            })
        );
    }

    function _validateClaim(
        SolventCrossChainOrder calldata order,
        SolventMandate calldata mandate,
        Claim calldata compactClaim
    )
        private
        view
    {
        require(
            compactClaim.sponsor == order.user && compactClaim.id == order.compactId
                && compactClaim.expires == order.compactExpires
                && compactClaim.witness == CrossChainHashLib.hashMandate(mandate)
                && keccak256(bytes(compactClaim.witnessTypestring)) == keccak256(bytes(MANDATE_WITNESS_TYPESTRING))
                && compactClaim.allocatedAmount == order.inputAmount && compactClaim.claimants.length == 1
                && compactClaim.claimants[0].claimant == uint256(uint160(address(this)))
                && compactClaim.claimants[0].amount == order.inputAmount,
            InvalidCompactClaim()
        );
    }
}
