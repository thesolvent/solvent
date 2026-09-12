// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

interface IRepaymentProofVerifier {
    struct VerifiedRepayment {
        bytes32 orderId;
        uint256 originChainId;
        address originSettler;
        address maker;
        address repaymentToken;
        uint256 repaymentAmount;
        bytes32 repaymentId;
    }

    function verifyRepayment(bytes calldata proof) external view returns (VerifiedRepayment memory repayment);
}
