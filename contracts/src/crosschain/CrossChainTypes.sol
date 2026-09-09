// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

enum RouteKind {
    TwoMakerCredit,
    DirectMaker
}

/// @notice The cross-chain order whose hash ties the Compact commitment to destination execution.
struct SolventCrossChainOrder {
    address user;
    uint256 nonce;
    uint256 originChainId;
    address originSettler;
    address compact;
    uint256 compactId;
    uint256 compactExpires;
    address inputToken;
    uint256 inputAmount;
    uint256 destinationChainId;
    address outputToken;
    uint256 minimumOutputAmount;
    address recipient;
    address destinationSettler;
    address fillProofVerifier;
    address exclusiveFiller;
    uint48 exclusivityEnds;
    uint48 fillDeadline;
    RouteKind routeKind;
}

/// @notice The portion of an order embedded as The Compact's witness.
struct SolventMandate {
    bytes32 orderId;
    uint256 destinationChainId;
    address destinationSettler;
    address fillProofVerifier;
    address outputToken;
    uint256 minimumOutputAmount;
    address recipient;
    uint48 fillDeadline;
    address exclusiveFiller;
    RouteKind routeKind;
}

/// @notice M1's authorization to deliver the destination asset now for the locked origin asset later.
struct DirectMakerQuote {
    bytes32 orderId;
    address maker;
    bytes32 destinationStrategyHash;
    bytes32 originStrategyHash;
    uint256 outputAmount;
    uint256 repaymentAmount;
    uint256 nonce;
    uint48 expires;
}

library CrossChainHashLib {
    bytes32 internal constant ORDER_TYPEHASH = keccak256(
        "SolventCrossChainOrder(address user,uint256 nonce,uint256 originChainId,address originSettler,address compact,uint256 compactId,uint256 compactExpires,address inputToken,uint256 inputAmount,uint256 destinationChainId,address outputToken,uint256 minimumOutputAmount,address recipient,address destinationSettler,address fillProofVerifier,address exclusiveFiller,uint48 exclusivityEnds,uint48 fillDeadline,uint8 routeKind)"
    );
    bytes32 internal constant MANDATE_TYPEHASH = keccak256(
        "Mandate(bytes32 orderId,uint256 destinationChainId,address destinationSettler,address fillProofVerifier,address outputToken,uint256 minimumOutputAmount,address recipient,uint48 fillDeadline,address exclusiveFiller,uint8 routeKind)"
    );
    bytes32 internal constant DIRECT_QUOTE_TYPEHASH = keccak256(
        "DirectMakerQuote(bytes32 orderId,address maker,bytes32 destinationStrategyHash,bytes32 originStrategyHash,uint256 outputAmount,uint256 repaymentAmount,uint256 nonce,uint48 expires)"
    );

    function hashOrder(SolventCrossChainOrder memory order) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                ORDER_TYPEHASH,
                order.user,
                order.nonce,
                order.originChainId,
                order.originSettler,
                order.compact,
                order.compactId,
                order.compactExpires,
                order.inputToken,
                order.inputAmount,
                order.destinationChainId,
                order.outputToken,
                order.minimumOutputAmount,
                order.recipient,
                order.destinationSettler,
                order.fillProofVerifier,
                order.exclusiveFiller,
                order.exclusivityEnds,
                order.fillDeadline,
                uint8(order.routeKind)
            )
        );
    }

    function hashMandate(SolventMandate memory mandate) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                MANDATE_TYPEHASH,
                mandate.orderId,
                mandate.destinationChainId,
                mandate.destinationSettler,
                mandate.fillProofVerifier,
                mandate.outputToken,
                mandate.minimumOutputAmount,
                mandate.recipient,
                mandate.fillDeadline,
                mandate.exclusiveFiller,
                uint8(mandate.routeKind)
            )
        );
    }

    function hashDirectQuote(DirectMakerQuote memory quote) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                DIRECT_QUOTE_TYPEHASH,
                quote.orderId,
                quote.maker,
                quote.destinationStrategyHash,
                quote.originStrategyHash,
                quote.outputAmount,
                quote.repaymentAmount,
                quote.nonce,
                quote.expires
            )
        );
    }
}
