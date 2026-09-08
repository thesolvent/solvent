// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import { Script, console2 } from "forge-std/Script.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { SameChainSettler } from "../src/SameChainSettler.sol";
import { GaslessCrossChainOrder } from "../src/interfaces/ERC7683.sol";

/// Emits a deterministic ERC-7683 order fixture (abi-encoded payload, the settler's own `orderId`, and
/// the Permit2 witness digest) so the Rust codec can be tested against the actual contract hashing.
/// The id comes from a real `SameChainSettler`, not a re-derivation, so the fixture cannot drift from
/// the contract. Run:
///   forge script script/GenSolventOrderFixture.s.sol -vvv
contract GenSolventOrderFixture is Script {
    function run() external {
        // Permit2 is only stored by the constructor; hashing does not touch it.
        SameChainSettler settler = new SameChainSettler(ISignatureTransfer(address(0)));

        SameChainSettler.SolventOrder memory inner = SameChainSettler.SolventOrder({
            inputToken: address(0x6666666666666666666666666666666666666666),
            inputAmount: 500e18,
            outputToken: address(0x7777777777777777777777777777777777777777),
            outputAmount: 1000e18,
            recipient: address(0x8888888888888888888888888888888888888888),
            exclusiveFiller: address(0x5555555555555555555555555555555555555555),
            exclusivityEnds: 1000
        });

        GaslessCrossChainOrder memory order = GaslessCrossChainOrder({
            originSettler: address(0x2222222222222222222222222222222222222222),
            user: address(0x3333333333333333333333333333333333333333),
            nonce: 42,
            originChainId: 1,
            openDeadline: 1500,
            fillDeadline: 2000,
            orderDataType: settler.SOLVENT_ORDER_TYPE_HASH(),
            orderData: abi.encode(inner)
        });

        bytes32 orderId = settler.orderIdFor(order);

        console2.log("PAYLOAD");
        console2.logBytes(abi.encode(order));
        console2.log("ORDER_DATA_TYPE");
        console2.logBytes32(settler.SOLVENT_ORDER_TYPE_HASH());
        console2.log("ORDER_ID");
        console2.logBytes32(orderId);

        // Swapper Permit2 EIP-712 witness digest, against canonical Permit2 on chainId 1.
        address permit2 = 0x000000000022D473030F116dDEE9F6B43aC78BA3;
        bytes32 domainSep = keccak256(
            abi.encode(
                keccak256("EIP712Domain(string name,uint256 chainId,address verifyingContract)"),
                keccak256("Permit2"),
                uint256(1),
                permit2
            )
        );
        bytes32 witnessTypeHash = keccak256(
            abi.encodePacked(
                "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,",
                "GaslessCrossChainOrder witness)",
                "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,uint256 originChainId,",
                "uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,bytes orderData)",
                "TokenPermissions(address token,uint256 amount)"
            )
        );
        bytes32 tpHash = keccak256(
            abi.encode(keccak256("TokenPermissions(address token,uint256 amount)"), inner.inputToken, inner.inputAmount)
        );
        bytes32 structHash = keccak256(
            abi.encode(witnessTypeHash, tpHash, order.originSettler, order.nonce, uint256(order.openDeadline), orderId)
        );
        console2.log("WITNESS_DIGEST");
        console2.logBytes32(keccak256(abi.encodePacked(hex"1901", domainSep, structHash)));
    }
}
