// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { ProofKind } from "../crosschain/ProofTypes.sol";

interface IProofOutbox {
    function record(bytes32 orderId, ProofKind kind, bytes calldata payload) external returns (bytes32 payloadHash);
}
