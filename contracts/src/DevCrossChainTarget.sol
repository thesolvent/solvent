// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @notice No-op target used only by the local backend saga harness.
/// @dev It accepts every staged command and reports zero CCIP fee; it does not move tokens or
///      provide settlement proofs, so it must never be deployed outside an ephemeral devnet.
contract DevCrossChainTarget {
    event Command(bytes4 indexed selector, bytes payload);

    function fee(bytes calldata envelope) external pure returns (uint256) {
        envelope;
        return 0;
    }

    function dispatch(bytes32 orderId, bytes calldata envelope) external returns (bytes32 messageId) {
        emit Command(msg.sig, abi.encode(orderId, envelope));
        return keccak256(abi.encode(orderId, envelope, block.number));
    }

    fallback() external payable {
        emit Command(msg.sig, msg.data);
    }

    receive() external payable {
        emit Command(bytes4(0), "");
    }
}
