// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Script } from "forge-std/Script.sol";
import { IAqua } from "@1inch/aqua/src/interfaces/IAqua.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { TheCompact } from "the-compact/src/TheCompact.sol";
import { AlwaysOKAllocator } from "the-compact/src/test/AlwaysOKAllocator.sol";
import { ResetPeriod } from "the-compact/src/types/ResetPeriod.sol";
import { Scope } from "the-compact/src/types/Scope.sol";

import { CompactOriginSettler } from "../src/CompactOriginSettler.sol";
import { CrossChainAquaApp } from "../src/CrossChainAquaApp.sol";
import { DevCcipRouter } from "../src/devnet/DevCcipRouter.sol";
import { CcipProofInbox } from "../src/proof/CcipProofInbox.sol";
import { CcipProofOutbox } from "../src/proof/CcipProofOutbox.sol";
import { IMessageTransmitterV2, ITokenMessengerV2 } from "../src/interfaces/ICctpV2.sol";
import { IWETH } from "../src/interfaces/IWETH.sol";

contract DeployCrossChainInfra is Script {
    function run() external {
        uint64 sourceSelector = uint64(vm.envUint("CCIP_REMOTE_CHAIN_SELECTOR"));
        uint64 destinationSelector = uint64(vm.envUint("CCIP_DESTINATION_CHAIN_SELECTOR"));

        vm.startBroadcast();
        DevCcipRouter router = new DevCcipRouter();
        CcipProofInbox inbox = new CcipProofInbox(address(router), sourceSelector, address(0));
        CcipProofOutbox outbox = new CcipProofOutbox(router, address(0), destinationSelector, address(0), 500_000);
        vm.stopBroadcast();

        string memory root = "crosschain-infra";
        vm.serializeUint(root, "chain_id", block.chainid);
        vm.serializeAddress(root, "router", address(router));
        vm.serializeAddress(root, "inbox", address(inbox));
        string memory json = vm.serializeAddress(root, "outbox", address(outbox));
        vm.writeJson(json, vm.envString("CROSSCHAIN_MANIFEST"));
    }
}

contract DeployOriginCompact is Script {
    function run() external {
        vm.startBroadcast();
        TheCompact compact = new TheCompact();
        AlwaysOKAllocator allocator = new AlwaysOKAllocator();
        uint96 allocatorId = compact.__registerAllocator(address(allocator), "");
        vm.stopBroadcast();

        bytes12 lockTag = bytes12(
            bytes32(
                (uint256(Scope.Multichain) << 255) | (uint256(ResetPeriod.TenMinutes) << 252)
                    | (uint256(allocatorId) << 160)
            )
        );
        string memory root = "origin-compact";
        vm.serializeAddress(root, "compact", address(compact));
        vm.serializeAddress(root, "allocator", address(allocator));
        vm.serializeUint(root, "allocator_id", allocatorId);
        string memory json = vm.serializeBytes32(root, "lock_tag", bytes32(lockTag));
        vm.writeJson(json, vm.envString("COMPACT_MANIFEST"));
    }
}

contract DeployDestinationCrossChainApp is Script {
    function run() external returns (CrossChainAquaApp app) {
        address dummy = vm.envAddress("DEVNET_DUMMY");
        vm.startBroadcast();
        app = new CrossChainAquaApp(
            IAqua(vm.envAddress("DESTINATION_AQUA")),
            IWETH(vm.envAddress("DESTINATION_WETH")),
            vm.envUint("ORIGIN_CHAIN_ID"),
            vm.envAddress("ORIGIN_SETTLER"),
            vm.envAddress("ORIGIN_COMPACT"),
            vm.envAddress("ORIGIN_TOKEN"),
            vm.envAddress("DESTINATION_FILL_VERIFIER"),
            CcipProofOutbox(payable(vm.envAddress("DESTINATION_PROOF_OUTBOX"))),
            CcipProofInbox(vm.envAddress("DESTINATION_REPAYMENT_VERIFIER")),
            CrossChainAquaApp.CctpConfig({
                usdc: IERC20(vm.envAddress("DESTINATION_USDC")),
                messageTransmitter: IMessageTransmitterV2(dummy),
                originDomain: 1,
                destinationDomain: 2,
                originTokenMessenger: bytes32(uint256(uint160(dummy))),
                destinationTokenMessenger: dummy,
                originUsdc: vm.envAddress("ORIGIN_USDC"),
                messageVersion: 1,
                burnMessageVersion: 1,
                minFinalityThreshold: 1,
                feeRecipient: dummy
            })
        );
        vm.stopBroadcast();
        vm.writeJson(vm.serializeAddress("destination-app", "app", address(app)), vm.envString("APP_MANIFEST"));
    }
}

contract DeployOriginSettler is Script {
    function run() external returns (CompactOriginSettler settler) {
        address dummy = vm.envAddress("DEVNET_DUMMY");
        vm.startBroadcast();
        settler = new CompactOriginSettler(
            TheCompact(payable(vm.envAddress("ORIGIN_COMPACT"))),
            IAqua(vm.envAddress("ORIGIN_AQUA")),
            IERC20(vm.envAddress("ORIGIN_TOKEN")),
            vm.envUint("DESTINATION_CHAIN_ID"),
            vm.envAddress("DESTINATION_APP"),
            CcipProofInbox(vm.envAddress("ORIGIN_FILL_VERIFIER")),
            CcipProofOutbox(payable(vm.envAddress("ORIGIN_PROOF_OUTBOX"))),
            CompactOriginSettler.RoutedConfig({
                originUsdc: IERC20(vm.envAddress("ORIGIN_USDC")),
                swapRouter: ISwapVM(vm.envAddress("ORIGIN_SWAP_ROUTER")),
                tokenMessenger: ITokenMessengerV2(dummy),
                destinationUsdc: vm.envAddress("DESTINATION_USDC"),
                destinationDomain: 2,
                minFinalityThreshold: 1,
                feeRecipient: dummy
            })
        );
        vm.stopBroadcast();
        vm.writeJson(vm.serializeAddress("origin-settler", "settler", address(settler)), vm.envString("SETTLER_MANIFEST"));
    }
}
