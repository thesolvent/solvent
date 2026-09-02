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

        console2.log("PAYLOAD");
        console2.logBytes(abi.encode(order));
        console2.log("ORDERHASH");
        console2.logBytes32(order.hash());
    }
}
