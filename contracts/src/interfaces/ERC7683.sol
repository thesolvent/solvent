// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @dev The ERC-7683 v1 structs and interfaces, transcribed from the reference text at
///      https://www.erc7683.org/spec. v1 is the version protocols actually deployed; the text at
///      eips.ethereum.org was redesigned around resolvers in May 2026 and has no deployments.

/// @notice A cross-chain order signed offchain by the user and opened onchain by someone else.
struct GaslessCrossChainOrder {
    address originSettler;
    address user;
    uint256 nonce;
    uint256 originChainId;
    uint32 openDeadline;
    uint32 fillDeadline;
    bytes32 orderDataType;
    bytes orderData;
}

/// @notice A cross-chain order opened onchain by the user themselves.
struct OnchainCrossChainOrder {
    uint32 fillDeadline;
    bytes32 orderDataType;
    bytes orderData;
}

/// @notice The protocol-agnostic view of an order, so a filler can price it without a decoder.
struct ResolvedCrossChainOrder {
    address user;
    uint256 originChainId;
    uint32 openDeadline;
    uint32 fillDeadline;
    bytes32 orderId;
    Output[] maxSpent;
    Output[] minReceived;
    FillInstruction[] fillInstructions;
}

/// @dev `bytes32` rather than `address` so non-EVM destinations fit.
struct Output {
    bytes32 token;
    uint256 amount;
    bytes32 recipient;
    uint256 chainId;
}

struct FillInstruction {
    uint64 destinationChainId;
    bytes32 destinationSettler;
    bytes originData;
}

interface IOriginSettler {
    /// @notice Signals that an order has been opened and is available to fill.
    event Open(bytes32 indexed orderId, ResolvedCrossChainOrder resolvedOrder);

    function openFor(
        GaslessCrossChainOrder calldata order,
        bytes calldata signature,
        bytes calldata originFillerData
    )
        external;

    function open(OnchainCrossChainOrder calldata order) external;

    function resolveFor(
        GaslessCrossChainOrder calldata order,
        bytes calldata originFillerData
    )
        external
        view
        returns (ResolvedCrossChainOrder memory);

    function resolve(OnchainCrossChainOrder calldata order) external view returns (ResolvedCrossChainOrder memory);
}

interface IDestinationSettler {
    /// @notice Fills a single leg of an order on the destination chain.
    function fill(bytes32 orderId, bytes calldata originData, bytes calldata fillerData) external;
}
