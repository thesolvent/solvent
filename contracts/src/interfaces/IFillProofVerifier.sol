// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { RouteKind } from "../crosschain/CrossChainTypes.sol";

interface IFillProofVerifier {
    struct VerifiedFill {
        bytes32 orderId;
        RouteKind routeKind;
        uint256 destinationChainId;
        address destinationApp;
        address recipient;
        address outputToken;
        uint256 outputAmount;
        address destinationMaker;
        bytes32 destinationStrategyHash;
        bytes32 originStrategyHash;
        address repaymentToken;
        uint256 repaymentAmount;
        bytes32 makerQuoteHash;
        bytes32 fillId;
    }

    function verifyFill(bytes calldata proof) external view returns (VerifiedFill memory fill);
}
