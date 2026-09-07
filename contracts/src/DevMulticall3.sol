// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @notice Multicall3-compatible aggregator for the devnet. Mainnet chains already host the
///         canonical deployment; a fresh anvil does not, and alloy's multicall builder reads the
///         canonical address, so the devnet places this implementation there instead. The full
///         interface is implemented — a missing selector reverts with empty data, which surfaces
///         far from its cause.
contract DevMulticall3 {
    struct Call {
        address target;
        bytes callData;
    }

    struct Call3 {
        address target;
        bool allowFailure;
        bytes callData;
    }

    struct Call3Value {
        address target;
        bool allowFailure;
        uint256 value;
        bytes callData;
    }

    struct Result {
        bool success;
        bytes returnData;
    }

    function aggregate(Call[] calldata calls) public payable returns (uint256 blockNumber, bytes[] memory returnData) {
        blockNumber = block.number;
        uint256 length = calls.length;
        returnData = new bytes[](length);
        for (uint256 i = 0; i < length; i++) {
            (bool success, bytes memory ret) = calls[i].target.call(calls[i].callData);
            if (!success) {
                _bubble(ret);
            }
            returnData[i] = ret;
        }
    }

    function aggregate3(Call3[] calldata calls) public payable returns (Result[] memory returnData) {
        uint256 length = calls.length;
        returnData = new Result[](length);
        for (uint256 i = 0; i < length; i++) {
            Result memory result = returnData[i];
            Call3 calldata call = calls[i];
            (result.success, result.returnData) = call.target.call(call.callData);
            if (!result.success && !call.allowFailure) {
                _bubble(result.returnData);
            }
        }
    }

    function aggregate3Value(Call3Value[] calldata calls) public payable returns (Result[] memory returnData) {
        uint256 length = calls.length;
        uint256 valueAccumulator;
        returnData = new Result[](length);
        for (uint256 i = 0; i < length; i++) {
            Result memory result = returnData[i];
            Call3Value calldata call = calls[i];
            valueAccumulator += call.value;
            (result.success, result.returnData) = call.target.call{ value: call.value }(call.callData);
            if (!result.success && !call.allowFailure) {
                _bubble(result.returnData);
            }
        }
        require(msg.value == valueAccumulator, "Multicall3: value mismatch");
    }

    function tryAggregate(
        bool requireSuccess,
        Call[] calldata calls
    )
        public
        payable
        returns (Result[] memory returnData)
    {
        uint256 length = calls.length;
        returnData = new Result[](length);
        for (uint256 i = 0; i < length; i++) {
            Result memory result = returnData[i];
            (result.success, result.returnData) = calls[i].target.call(calls[i].callData);
            if (requireSuccess && !result.success) {
                _bubble(result.returnData);
            }
        }
    }

    function tryBlockAndAggregate(
        bool requireSuccess,
        Call[] calldata calls
    )
        public
        payable
        returns (uint256 blockNumber, bytes32 blockHash, Result[] memory returnData)
    {
        blockNumber = block.number;
        blockHash = blockhash(block.number);
        returnData = tryAggregate(requireSuccess, calls);
    }

    function blockAndAggregate(Call[] calldata calls)
        public
        payable
        returns (uint256 blockNumber, bytes32 blockHash, Result[] memory returnData)
    {
        (blockNumber, blockHash, returnData) = tryBlockAndAggregate(true, calls);
    }

    function getBasefee() external view returns (uint256) {
        return block.basefee;
    }

    function getBlockHash(uint256 blockNumber) external view returns (bytes32) {
        return blockhash(blockNumber);
    }

    function getBlockNumber() external view returns (uint256) {
        return block.number;
    }

    function getChainId() external view returns (uint256) {
        return block.chainid;
    }

    function getCurrentBlockCoinbase() external view returns (address) {
        return block.coinbase;
    }

    function getCurrentBlockDifficulty() external view returns (uint256) {
        return block.prevrandao;
    }

    function getCurrentBlockGasLimit() external view returns (uint256) {
        return block.gaslimit;
    }

    function getCurrentBlockTimestamp() external view returns (uint256) {
        return block.timestamp;
    }

    function getEthBalance(address addr) external view returns (uint256) {
        return addr.balance;
    }

    function getLastBlockHash() external view returns (bytes32) {
        return blockhash(block.number - 1);
    }

    /// Re-raise the callee's revert data so failures read the same as a direct call.
    function _bubble(bytes memory returnData) private pure {
        if (returnData.length == 0) {
            revert("Multicall3: call failed");
        }
        assembly ("memory-safe") {
            revert(add(returnData, 0x20), mload(returnData))
        }
    }
}
