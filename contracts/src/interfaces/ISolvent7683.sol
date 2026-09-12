// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// @notice Same-chain order signed through Permit2 and described by Solvent's ERC-7683 resolver.
struct Solvent7683Order {
    address settler;
    address user;
    uint256 chainId;
    address inputToken;
    uint256 inputAmount;
    address outputToken;
    uint256 outputAmount;
    address recipient;
    uint256 executorFee;
    uint256 nonce;
    uint256 deadline;
}

interface ISolventSameChainSettler {
    function validate(Solvent7683Order calldata order) external view returns (bytes32 orderId);

    function orderId(Solvent7683Order calldata order) external pure returns (bytes32);

    function settle(Solvent7683Order calldata order, bytes calldata permitSignature) external;
}

interface IErc7683AquaFiller {
    function settler() external view returns (address);

    function fill(
        bytes calldata orderData,
        bytes calldata permitSignature,
        bytes calldata sourceData,
        address paymentRecipient
    )
        external
        returns (uint256 earned);
}
