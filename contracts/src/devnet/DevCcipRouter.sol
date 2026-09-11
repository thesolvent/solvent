// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IAny2EVMMessageReceiver } from "@chainlink/contracts-ccip/contracts/interfaces/IAny2EVMMessageReceiver.sol";
import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

/// @notice Local-only CCIP boundary. A relay forwards queued payloads to the other Anvil chain.
contract DevCcipRouter is IRouterClient {
    uint256 public constant FEE = 0.001 ether;
    uint256 private _nonce;

    event MessageQueued(
        bytes32 indexed messageId,
        uint64 indexed destinationSelector,
        address indexed sourceOutbox,
        address receiver,
        bytes payload
    );

    function isChainSupported(uint64) external pure returns (bool) {
        return true;
    }

    function getFee(uint64, Client.EVM2AnyMessage memory) external pure returns (uint256) {
        return FEE;
    }

    function ccipSend(
        uint64 selector,
        Client.EVM2AnyMessage calldata message
    )
        external
        payable
        returns (bytes32 messageId)
    {
        require(msg.value == FEE, "fee");
        address receiver = abi.decode(message.receiver, (address));
        messageId = keccak256(abi.encode(block.chainid, msg.sender, selector, ++_nonce, message.data));
        emit MessageQueued(messageId, selector, msg.sender, receiver, message.data);
    }

    function deliverPayload(
        bytes32 messageId,
        uint64 sourceSelector,
        address sourceOutbox,
        address receiver,
        bytes calldata payload
    )
        external
    {
        Client.EVMTokenAmount[] memory noTokens = new Client.EVMTokenAmount[](0);
        Client.Any2EVMMessage memory message = Client.Any2EVMMessage({
            messageId: messageId,
            sourceChainSelector: sourceSelector,
            sender: abi.encode(sourceOutbox),
            data: payload,
            destTokenAmounts: noTokens
        });
        IAny2EVMMessageReceiver(receiver).ccipReceive(message);
    }
}
