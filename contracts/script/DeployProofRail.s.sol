// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script } from "forge-std/Script.sol";
import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";

import { CcipProofInbox } from "../src/proof/CcipProofInbox.sol";
import { CcipProofOutbox } from "../src/proof/CcipProofOutbox.sol";

/// @notice Deploys one chain's proof endpoints. Unknown recorder/remote addresses are initialized later.
contract DeployProofRail is Script {
    function run() external returns (CcipProofInbox inbox, CcipProofOutbox outbox) {
        address router = vm.envAddress("CCIP_ROUTER");
        uint64 localRemoteSelector = uint64(vm.envUint("CCIP_REMOTE_CHAIN_SELECTOR"));
        uint64 destinationSelector = uint64(vm.envUint("CCIP_DESTINATION_CHAIN_SELECTOR"));
        address remoteOutbox = vm.envOr("CCIP_REMOTE_OUTBOX", address(0));
        address recorder = vm.envOr("PROOF_RECORDER", address(0));
        address destinationInbox = vm.envOr("CCIP_DESTINATION_INBOX", address(0));
        uint256 gasLimit = vm.envOr("CCIP_DESTINATION_GAS_LIMIT", uint256(500_000));

        vm.startBroadcast();
        inbox = new CcipProofInbox(router, localRemoteSelector, remoteOutbox);
        outbox = new CcipProofOutbox(IRouterClient(router), recorder, destinationSelector, destinationInbox, gasLimit);
        vm.stopBroadcast();

        string memory root = "proof-rail";
        vm.serializeUint(root, "chain_id", block.chainid);
        vm.serializeAddress(root, "inbox", address(inbox));
        string memory json = vm.serializeAddress(root, "outbox", address(outbox));
        vm.writeJson(json, vm.envOr("PROOF_MANIFEST", string("deployments/proof-rail.json")));
    }
}

/// @notice Locks the two one-time deployment links after both chain manifests are available.
contract ConfigureProofRail is Script {
    function run() external {
        CcipProofInbox inbox = CcipProofInbox(vm.envAddress("PROOF_INBOX"));
        CcipProofOutbox outbox = CcipProofOutbox(vm.envAddress("PROOF_OUTBOX"));
        address remoteOutbox = vm.envAddress("CCIP_REMOTE_OUTBOX");
        address recorder = vm.envAddress("PROOF_RECORDER");
        address destinationInbox = vm.envAddress("CCIP_DESTINATION_INBOX");

        vm.startBroadcast();
        if (inbox.sourceOutbox() == address(0)) {
            inbox.initializeSourceOutbox(remoteOutbox);
        }
        if (outbox.recorder() == address(0)) {
            outbox.initializeRecorder(recorder);
        }
        if (outbox.destinationInbox() == address(0)) {
            outbox.initializeDestinationInbox(destinationInbox);
        }
        vm.stopBroadcast();
    }
}
