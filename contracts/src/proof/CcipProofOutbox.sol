// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

import { ProofHashLib, ProofKind } from "../crosschain/ProofTypes.sol";
import { IProofOutbox } from "../interfaces/IProofOutbox.sol";

/// @notice Records settlement evidence synchronously and dispatches the exact bytes later through CCIP.
contract CcipProofOutbox is IProofOutbox {
    IRouterClient public immutable ROUTER;
    address public immutable DEPLOYER;
    address public recorder;
    uint64 public immutable DESTINATION_CHAIN_SELECTOR;
    address public destinationInbox;
    uint256 public immutable DESTINATION_GAS_LIMIT;

    mapping(bytes32 orderId => bytes32 payloadHash) public recordedPayloads;

    event ProofRecorded(bytes32 indexed orderId, ProofKind indexed kind, bytes32 indexed payloadHash, bytes envelope);
    event ProofDispatched(bytes32 indexed orderId, bytes32 indexed payloadHash, bytes32 indexed messageId);

    error IncorrectFee(uint256 expected, uint256 supplied);
    error DestinationInboxAlreadyInitialized();
    error InvalidEnvelope();
    error NotRecorder(address caller);
    error RecorderAlreadyInitialized();
    error PayloadConflict(bytes32 orderId, bytes32 recorded, bytes32 supplied);
    error UnknownPayload(bytes32 orderId);
    error ZeroAddress();

    constructor(
        IRouterClient router,
        address initialRecorder,
        uint64 destinationChainSelector,
        address initialDestinationInbox,
        uint256 destinationGasLimit
    ) {
        if (address(router) == address(0)) {
            revert ZeroAddress();
        }
        ROUTER = router;
        DEPLOYER = msg.sender;
        recorder = initialRecorder;
        DESTINATION_CHAIN_SELECTOR = destinationChainSelector;
        destinationInbox = initialDestinationInbox;
        DESTINATION_GAS_LIMIT = destinationGasLimit;
    }

    /// @notice Completes two-chain deployments once the remote inbox address is known.
    function initializeDestinationInbox(address remoteInbox) external {
        if (msg.sender != DEPLOYER) {
            revert NotRecorder(msg.sender);
        }
        if (destinationInbox != address(0)) {
            revert DestinationInboxAlreadyInitialized();
        }
        if (remoteInbox == address(0)) {
            revert ZeroAddress();
        }
        destinationInbox = remoteInbox;
    }

    /// @notice Completes deployments where the recorder needs the outbox address in its constructor.
    function initializeRecorder(address authorizedRecorder) external {
        if (msg.sender != DEPLOYER) {
            revert NotRecorder(msg.sender);
        }
        if (recorder != address(0)) {
            revert RecorderAlreadyInitialized();
        }
        if (authorizedRecorder == address(0)) {
            revert ZeroAddress();
        }
        recorder = authorizedRecorder;
    }

    function record(bytes32 orderId, ProofKind kind, bytes calldata payload) external returns (bytes32 payloadHash) {
        if (msg.sender != recorder) {
            revert NotRecorder(msg.sender);
        }
        if (orderId == bytes32(0) || payload.length == 0) {
            revert InvalidEnvelope();
        }

        bytes memory envelope = ProofHashLib.encode(kind, payload);
        payloadHash = keccak256(envelope);
        bytes32 recorded = recordedPayloads[orderId];
        if (recorded != bytes32(0) && recorded != payloadHash) {
            revert PayloadConflict(orderId, recorded, payloadHash);
        }
        if (recorded == bytes32(0)) {
            recordedPayloads[orderId] = payloadHash;
            emit ProofRecorded(orderId, kind, payloadHash, envelope);
        }
    }

    function fee(bytes calldata envelope) external view returns (uint256) {
        return ROUTER.getFee(DESTINATION_CHAIN_SELECTOR, _message(envelope));
    }

    function dispatch(bytes32 orderId, bytes calldata envelope) external payable returns (bytes32 messageId) {
        bytes32 payloadHash = recordedPayloads[orderId];
        if (payloadHash == bytes32(0)) {
            revert UnknownPayload(orderId);
        }
        if (keccak256(envelope) != payloadHash) {
            revert PayloadConflict(orderId, payloadHash, keccak256(envelope));
        }

        Client.EVM2AnyMessage memory message = _message(envelope);
        uint256 expectedFee = ROUTER.getFee(DESTINATION_CHAIN_SELECTOR, message);
        if (msg.value != expectedFee) {
            revert IncorrectFee(expectedFee, msg.value);
        }
        messageId = ROUTER.ccipSend{ value: expectedFee }(DESTINATION_CHAIN_SELECTOR, message);
        emit ProofDispatched(orderId, payloadHash, messageId);
    }

    function _message(bytes calldata envelope) private view returns (Client.EVM2AnyMessage memory) {
        if (destinationInbox == address(0)) {
            revert ZeroAddress();
        }
        Client.EVMTokenAmount[] memory noTokens = new Client.EVMTokenAmount[](0);
        return Client.EVM2AnyMessage({
            receiver: abi.encode(destinationInbox),
            data: envelope,
            tokenAmounts: noTokens,
            feeToken: address(0),
            extraArgs: Client._argsToBytes(
                Client.GenericExtraArgsV2({ gasLimit: DESTINATION_GAS_LIMIT, allowOutOfOrderExecution: true })
            )
        });
    }
}
