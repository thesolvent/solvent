// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { CCIPReceiver } from "@chainlink/contracts-ccip/contracts/applications/CCIPReceiver.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

import { ProofEnvelope, ProofHashLib, ProofKind } from "../crosschain/ProofTypes.sol";
import { IFillProofVerifier } from "../interfaces/IFillProofVerifier.sol";
import { IRepaymentProofVerifier } from "../interfaces/IRepaymentProofVerifier.sol";

/// @notice Authenticates one remote CCIP outbox and exposes its typed proofs to settlement contracts.
contract CcipProofInbox is CCIPReceiver, IFillProofVerifier, IRepaymentProofVerifier {
    uint64 public immutable SOURCE_CHAIN_SELECTOR;
    address public immutable DEPLOYER;
    address public sourceOutbox;

    mapping(bytes32 messageId => bool) public consumedMessages;
    mapping(bytes32 orderId => bytes32 fillId) public fillForOrder;
    mapping(bytes32 fillId => VerifiedFill fill) private _fills;
    mapping(bytes32 orderId => bytes32 repaymentId) public repaymentForOrder;
    mapping(bytes32 repaymentId => VerifiedRepayment repayment) private _repayments;

    event FillProofReceived(bytes32 indexed orderId, bytes32 indexed fillId, bytes32 indexed messageId);
    event RepaymentProofReceived(bytes32 indexed orderId, bytes32 indexed repaymentId, bytes32 indexed messageId);

    error DuplicateMessage(bytes32 messageId);
    error InvalidEnvelope();
    error InvalidProof();
    error InvalidRemote(uint64 sourceSelector, address sourceOutbox);
    error SourceOutboxAlreadyInitialized();
    error ProofConflict(bytes32 orderId);
    error UnknownProof(bytes32 proofId);
    error ZeroAddress();

    constructor(address router, uint64 sourceChainSelector, address initialSourceOutbox) CCIPReceiver(router) {
        SOURCE_CHAIN_SELECTOR = sourceChainSelector;
        DEPLOYER = msg.sender;
        sourceOutbox = initialSourceOutbox;
    }

    /// @notice Completes two-chain deployments once the remote outbox address is known.
    function initializeSourceOutbox(address remoteOutbox) external {
        if (msg.sender != DEPLOYER) {
            revert InvalidRemote(SOURCE_CHAIN_SELECTOR, msg.sender);
        }
        if (sourceOutbox != address(0)) {
            revert SourceOutboxAlreadyInitialized();
        }
        if (remoteOutbox == address(0)) {
            revert ZeroAddress();
        }
        sourceOutbox = remoteOutbox;
    }

    function verifyFill(bytes calldata proof) external view returns (VerifiedFill memory fill) {
        bytes32 fillId = _proofId(proof);
        fill = _fills[fillId];
        if (fill.fillId == bytes32(0)) {
            revert UnknownProof(fillId);
        }
    }

    function verifyRepayment(bytes calldata proof) external view returns (VerifiedRepayment memory repayment) {
        bytes32 repaymentId = _proofId(proof);
        repayment = _repayments[repaymentId];
        if (repayment.repaymentId == bytes32(0)) {
            revert UnknownProof(repaymentId);
        }
    }

    function _ccipReceive(Client.Any2EVMMessage memory message) internal override {
        if (consumedMessages[message.messageId]) {
            revert DuplicateMessage(message.messageId);
        }
        if (message.sourceChainSelector != SOURCE_CHAIN_SELECTOR || message.sender.length != 32) {
            revert InvalidRemote(message.sourceChainSelector, address(0));
        }
        address remoteOutbox = abi.decode(message.sender, (address));
        if (remoteOutbox != sourceOutbox || message.destTokenAmounts.length != 0) {
            revert InvalidRemote(message.sourceChainSelector, remoteOutbox);
        }

        ProofEnvelope memory envelope = abi.decode(message.data, (ProofEnvelope));
        if (envelope.version != ProofHashLib.VERSION || envelope.payload.length == 0) {
            revert InvalidEnvelope();
        }
        consumedMessages[message.messageId] = true;

        if (envelope.kind == ProofKind.Fill) {
            _storeFill(abi.decode(envelope.payload, (VerifiedFill)), message.messageId);
        } else if (envelope.kind == ProofKind.DirectRepayment) {
            _storeRepayment(abi.decode(envelope.payload, (VerifiedRepayment)), message.messageId);
        } else {
            revert InvalidEnvelope();
        }
    }

    function _storeFill(VerifiedFill memory fill, bytes32 messageId) private {
        if (fill.orderId == bytes32(0) || fill.fillId == bytes32(0)) {
            revert InvalidProof();
        }
        if (fillForOrder[fill.orderId] != bytes32(0) || _fills[fill.fillId].fillId != bytes32(0)) {
            revert ProofConflict(fill.orderId);
        }
        fillForOrder[fill.orderId] = fill.fillId;
        _fills[fill.fillId] = fill;
        emit FillProofReceived(fill.orderId, fill.fillId, messageId);
    }

    function _storeRepayment(VerifiedRepayment memory repayment, bytes32 messageId) private {
        if (repayment.orderId == bytes32(0) || repayment.repaymentId == bytes32(0)) {
            revert InvalidProof();
        }
        if (
            repaymentForOrder[repayment.orderId] != bytes32(0)
                || _repayments[repayment.repaymentId].repaymentId != bytes32(0)
        ) {
            revert ProofConflict(repayment.orderId);
        }
        repaymentForOrder[repayment.orderId] = repayment.repaymentId;
        _repayments[repayment.repaymentId] = repayment;
        emit RepaymentProofReceived(repayment.orderId, repayment.repaymentId, messageId);
    }

    function _proofId(bytes calldata proof) private pure returns (bytes32 proofId) {
        if (proof.length != 32) {
            revert InvalidProof();
        }
        proofId = abi.decode(proof, (bytes32));
    }
}
