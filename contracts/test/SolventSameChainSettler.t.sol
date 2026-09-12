// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";

import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { SolventSameChainSettler } from "../src/SolventSameChainSettler.sol";
import { SolventTakerCredential } from "../src/SolventTakerCredential.sol";
import { Solvent7683Order } from "../src/interfaces/ISolvent7683.sol";

contract SettlerTakerMock {
    SolventSameChainSettler internal immutable SETTLER;

    constructor(SolventSameChainSettler settler_) {
        SETTLER = settler_;
    }

    function settle(Solvent7683Order calldata order, bytes calldata signature) external {
        TokenMock(order.outputToken).approve(address(SETTLER), order.outputAmount);
        SETTLER.settle(order, signature);
    }
}

contract SettlerCredentialPeer { }

contract SolventSameChainSettlerTest is Test, DeployPermit2 {
    bytes internal constant WITNESS_STUB =
        "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
    bytes32 internal constant TOKEN_PERMISSIONS_TYPEHASH = keccak256("TokenPermissions(address token,uint256 amount)");

    uint256 internal constant USER_KEY = 0x1010;
    uint256 internal constant INPUT_AMOUNT = 3100 ether;
    uint256 internal constant OUTPUT_AMOUNT = 1 ether;
    uint256 internal constant EXECUTOR_FEE = 10 ether;

    ISignatureTransfer internal permit2;
    SolventTakerCredential internal credential;
    SolventSameChainSettler internal settler;
    SettlerTakerMock internal taker;
    TokenMock internal inputToken;
    TokenMock internal outputToken;
    address internal user;

    function setUp() public {
        permit2 = ISignatureTransfer(deployPermit2());
        credential = new SolventTakerCredential(address(this));
        settler = new SolventSameChainSettler(permit2, credential);
        taker = new SettlerTakerMock(settler);
        credential.setTaker(address(taker), true);
        credential.setTaker(address(new SettlerCredentialPeer()), true);
        credential.freeze();

        inputToken = new TokenMock("Input", "IN");
        outputToken = new TokenMock("Output", "OUT");
        user = vm.addr(USER_KEY);
        inputToken.mint(user, INPUT_AMOUNT * 2);
        outputToken.mint(address(taker), OUTPUT_AMOUNT * 2);
        vm.prank(user);
        inputToken.approve(address(permit2), type(uint256).max);
        vm.warp(1_000_000);
    }

    function test_settleAtomicallyMovesExactSignedAssets() public {
        Solvent7683Order memory order = _order(0);
        taker.settle(order, _sign(order));

        assertEq(inputToken.balanceOf(user), INPUT_AMOUNT);
        assertEq(inputToken.balanceOf(address(taker)), INPUT_AMOUNT);
        assertEq(outputToken.balanceOf(user), OUTPUT_AMOUNT);
        assertEq(outputToken.balanceOf(address(taker)), OUTPUT_AMOUNT);
        assertEq(inputToken.balanceOf(address(settler)), 0);
        assertEq(outputToken.balanceOf(address(settler)), 0);
    }

    function test_permitNonceCannotReplay() public {
        Solvent7683Order memory order = _order(7);
        bytes memory signature = _sign(order);
        taker.settle(order, signature);

        vm.expectRevert();
        taker.settle(order, signature);
    }

    function test_signatureCannotAuthorizeChangedOrder() public {
        Solvent7683Order memory order = _order(8);
        bytes memory signature = _sign(order);
        order.outputAmount += 1;

        vm.expectRevert();
        taker.settle(order, signature);
    }

    function test_settleRejectsCallerWithoutFrozenCredential() public {
        Solvent7683Order memory order = _order(9);
        bytes memory signature = _sign(order);
        vm.expectRevert(abi.encodeWithSelector(SolventSameChainSettler.CredentialNotActive.selector, address(this)));
        settler.settle(order, signature);
    }

    function test_validateRejectsExpiredAndWrongChainOrders() public {
        Solvent7683Order memory order = _order(10);
        order.chainId = block.chainid + 1;
        vm.expectRevert(
            abi.encodeWithSelector(SolventSameChainSettler.WrongChain.selector, block.chainid + 1, block.chainid)
        );
        settler.validate(order);

        order.chainId = block.chainid;
        order.deadline = block.timestamp - 1;
        vm.expectRevert(
            abi.encodeWithSelector(
                SolventSameChainSettler.DeadlinePassed.selector, block.timestamp - 1, block.timestamp
            )
        );
        settler.validate(order);
    }

    function _order(uint256 nonce) internal view returns (Solvent7683Order memory) {
        return Solvent7683Order({
            settler: address(settler),
            user: user,
            chainId: block.chainid,
            inputToken: address(inputToken),
            inputAmount: INPUT_AMOUNT,
            outputToken: address(outputToken),
            outputAmount: OUTPUT_AMOUNT,
            recipient: user,
            executorFee: EXECUTOR_FEE,
            nonce: nonce,
            deadline: block.timestamp + 1 hours
        });
    }

    function _sign(Solvent7683Order memory order) internal view returns (bytes memory) {
        bytes32 tokenPermissions =
            keccak256(abi.encode(TOKEN_PERMISSIONS_TYPEHASH, order.inputToken, order.inputAmount));
        bytes32 witnessTypeHash = keccak256(abi.encodePacked(WITNESS_STUB, settler.WITNESS_TYPE_STRING()));
        bytes32 structHash = keccak256(
            abi.encode(
                witnessTypeHash, tokenPermissions, address(settler), order.nonce, order.deadline, settler.orderId(order)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", permit2.DOMAIN_SEPARATOR(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(USER_KEY, digest);
        return bytes.concat(r, s, bytes1(v));
    }
}
