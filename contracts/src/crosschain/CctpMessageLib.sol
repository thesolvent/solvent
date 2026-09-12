// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @notice Reads the fixed CCTP V2 header, burn body, and Solvent repayment hook.
/// @dev Offsets match Circle's evm-cctp-contracts revision a92a2b4e7e6ef99bf0b05dca71780f5ec190e729.
library CctpMessageLib {
    uint256 internal constant MESSAGE_LENGTH = 440;
    uint256 internal constant HOOK_VERSION = 1;

    struct ParsedMessage {
        uint32 version;
        uint32 sourceDomain;
        uint32 destinationDomain;
        bytes32 nonce;
        bytes32 sender;
        bytes32 recipient;
        bytes32 destinationCaller;
        uint32 minFinalityThreshold;
        uint32 finalityThresholdExecuted;
        uint32 burnVersion;
        bytes32 burnToken;
        bytes32 mintRecipient;
        uint256 amount;
        bytes32 messageSender;
        uint256 maxFee;
        uint256 feeExecuted;
        uint256 expirationBlock;
        uint256 hookVersion;
        bytes32 hookOrderId;
    }

    error InvalidMessageLength(uint256 actual, uint256 expected);

    function parse(bytes calldata message) internal pure returns (ParsedMessage memory parsed) {
        require(message.length == MESSAGE_LENGTH, InvalidMessageLength(message.length, MESSAGE_LENGTH));
        parsed.version = uint32(bytes4(message[0:4]));
        parsed.sourceDomain = uint32(bytes4(message[4:8]));
        parsed.destinationDomain = uint32(bytes4(message[8:12]));
        parsed.nonce = bytes32(message[12:44]);
        parsed.sender = bytes32(message[44:76]);
        parsed.recipient = bytes32(message[76:108]);
        parsed.destinationCaller = bytes32(message[108:140]);
        parsed.minFinalityThreshold = uint32(bytes4(message[140:144]));
        parsed.finalityThresholdExecuted = uint32(bytes4(message[144:148]));
        parsed.burnVersion = uint32(bytes4(message[148:152]));
        parsed.burnToken = bytes32(message[152:184]);
        parsed.mintRecipient = bytes32(message[184:216]);
        parsed.amount = uint256(bytes32(message[216:248]));
        parsed.messageSender = bytes32(message[248:280]);
        parsed.maxFee = uint256(bytes32(message[280:312]));
        parsed.feeExecuted = uint256(bytes32(message[312:344]));
        parsed.expirationBlock = uint256(bytes32(message[344:376]));
        parsed.hookVersion = uint256(bytes32(message[376:408]));
        parsed.hookOrderId = bytes32(message[408:440]);
    }

    function repaymentHook(bytes32 orderId) internal pure returns (bytes memory) {
        return abi.encode(HOOK_VERSION, orderId);
    }

    function addressToBytes32(address value) internal pure returns (bytes32) {
        return bytes32(uint256(uint160(value)));
    }
}
