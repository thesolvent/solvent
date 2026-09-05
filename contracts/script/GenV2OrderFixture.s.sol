// SPDX-License-Identifier: MIT
pragma solidity ^0.8.30;

import { Script, console2 } from "forge-std/Script.sol";
import { ERC20 } from "solmate/src/tokens/ERC20.sol";
import { V2DutchOrder, CosignerData, V2DutchOrderLib } from "../lib/UniswapX/src/lib/V2DutchOrderLib.sol";
import { DutchInput, DutchOutput } from "../lib/UniswapX/src/lib/DutchOrderLib.sol";
import { OrderInfo } from "../lib/UniswapX/src/base/ReactorStructs.sol";
import { IReactor } from "../lib/UniswapX/src/interfaces/IReactor.sol";
import { IValidationCallback } from "../lib/UniswapX/src/interfaces/IValidationCallback.sol";

/// Emits a deterministic V2 Dutch order fixture (abi-encoded payload + the real `orderHash`) so the
/// Rust normalizer can be tested against the actual contract hashing. Run:
///   forge script script/GenV2OrderFixture.s.sol -vvv
contract GenV2OrderFixture is Script {
    using V2DutchOrderLib for V2DutchOrder;

    function run() external pure {
        OrderInfo memory info = OrderInfo({
            reactor: IReactor(address(0x2222222222222222222222222222222222222222)),
            swapper: address(0x3333333333333333333333333333333333333333),
            nonce: 42,
            deadline: 2000,
            additionalValidationContract: IValidationCallback(address(0)),
            additionalValidationData: ""
        });

        DutchOutput[] memory outs = new DutchOutput[](1);
        outs[0] = DutchOutput({
            token: address(0x7777777777777777777777777777777777777777),
            startAmount: 1000e18,
            endAmount: 900e18,
            recipient: address(0x8888888888888888888888888888888888888888)
        });

        CosignerData memory cd = CosignerData({
            decayStartTime: 1000,
            decayEndTime: 1100,
            exclusiveFiller: address(0x5555555555555555555555555555555555555555),
            exclusivityOverrideBps: 0,
            inputAmount: 0,
            outputAmounts: new uint256[](0)
        });

        V2DutchOrder memory order = V2DutchOrder({
            info: info,
            cosigner: address(0x4444444444444444444444444444444444444444),
            baseInput: DutchInput({
                token: ERC20(address(0x6666666666666666666666666666666666666666)),
                startAmount: 500e18,
                endAmount: 500e18
            }),
            baseOutputs: outs,
            cosignerData: cd,
            cosignature: ""
        });

        bytes32 orderHash = order.hash();

        console2.log("PAYLOAD");
        console2.logBytes(abi.encode(order));
        console2.log("ORDERHASH");
        console2.logBytes32(orderHash);

        // Cosigner preimage: keccak(orderHash || abi.encode(cosignerData)), signed raw.
        console2.log("COSIGN_DIGEST");
        console2.logBytes32(keccak256(abi.encodePacked(orderHash, abi.encode(order.cosignerData))));

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
                V2DutchOrderLib.PERMIT2_ORDER_TYPE
            )
        );
        bytes32 tpHash = keccak256(
            abi.encode(
                keccak256("TokenPermissions(address token,uint256 amount)"),
                address(order.baseInput.token),
                order.baseInput.endAmount
            )
        );
        bytes32 structHash = keccak256(
            abi.encode(
                witnessTypeHash, tpHash, address(order.info.reactor), order.info.nonce, order.info.deadline, orderHash
            )
        );
        console2.log("WITNESS_DIGEST");
        console2.logBytes32(keccak256(abi.encodePacked(hex"1901", domainSep, structHash)));
    }
}
