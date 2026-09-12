// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { InteroperableAddress } from "@openzeppelin/contracts-erc7683/utils/draft-InteroperableAddress.sol";
import { IAttribute, IPayment, IResolver, IStep, IVariableRole } from "erc7683/ERC7683.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { Solvent7683Resolver } from "../src/Solvent7683Resolver.sol";
import { SolventSameChainSettler } from "../src/SolventSameChainSettler.sol";
import { SolventTakerCredential } from "../src/SolventTakerCredential.sol";
import { IErc7683AquaFiller, Solvent7683Order } from "../src/interfaces/ISolvent7683.sol";

contract ResolverFillerMock is IErc7683AquaFiller {
    address private immutable _SETTLER;

    constructor(address settler_) {
        _SETTLER = settler_;
    }

    function settler() external view returns (address) {
        return _SETTLER;
    }

    function fill(bytes calldata, bytes calldata, bytes calldata, address) external pure returns (uint256) {
        return 0;
    }
}

contract ResolverPermit2Mock { }

contract Erc7683Decoder {
    function step(bytes calldata encoded)
        external
        pure
        returns (bytes memory target, bytes4 selector, bytes[] memory arguments, bytes[] memory attributes)
    {
        require(bytes4(encoded[:4]) == IStep.Call.selector);
        return abi.decode(encoded[4:], (bytes, bytes4, bytes[], bytes[]));
    }

    function payment(bytes calldata encoded)
        external
        pure
        returns (
            bytes memory token,
            bytes memory sender,
            bytes memory amountFormula,
            uint256 recipientVariable,
            uint256 stepIndex,
            uint256 delay
        )
    {
        require(bytes4(encoded[:4]) == IPayment.ERC20.selector);
        return abi.decode(encoded[4:], (bytes, bytes, bytes, uint256, uint256, uint256));
    }

    function constantFormula(bytes calldata encoded) external pure returns (uint256) {
        require(bytes4(encoded[:4]) == bytes4(keccak256("Constant(uint256)")));
        return abi.decode(encoded[4:], (uint256));
    }

    function witness(bytes calldata encoded)
        external
        pure
        returns (string memory kind, bytes memory data, uint256[] memory variables)
    {
        require(bytes4(encoded[:4]) == IVariableRole.Witness.selector);
        return abi.decode(encoded[4:], (string, bytes, uint256[]));
    }
}

contract Solvent7683ResolverTest is Test {
    SolventSameChainSettler internal settler;
    ResolverFillerMock internal filler;
    Solvent7683Resolver internal resolver;
    Erc7683Decoder internal decoder;

    function setUp() public {
        SolventTakerCredential credential = new SolventTakerCredential(address(this));
        settler = new SolventSameChainSettler(ISignatureTransfer(address(new ResolverPermit2Mock())), credential);
        filler = new ResolverFillerMock(address(settler));
        resolver = new Solvent7683Resolver(settler, filler);
        decoder = new Erc7683Decoder();
        vm.warp(1_000_000);
    }

    function test_resolveDescribesTheAtomicFillerCallAndExecutorPayment() public view {
        Solvent7683Order memory solventOrder = _order();
        IResolver.ResolvedOrder memory resolved = resolver.resolve(abi.encode(solventOrder));

        assertEq(resolved.steps.length, 1);
        assertEq(resolved.variables.length, 4);
        assertEq(resolved.payments.length, 1);
        assertEq(resolved.assumptions.length, 0);

        (bytes memory target, bytes4 selector, bytes[] memory arguments, bytes[] memory attributes) =
            decoder.step(resolved.steps[0]);
        (uint256 targetChain, address targetAddress) = InteroperableAddress.parseEvmV1(target);
        assertEq(targetChain, block.chainid);
        assertEq(targetAddress, address(filler));
        assertEq(selector, IErc7683AquaFiller.fill.selector);
        assertEq(arguments.length, 4);
        (, bytes memory encodedOrder) = abi.decode(arguments[0], (string, bytes));
        assertEq(keccak256(encodedOrder), keccak256(abi.encode(solventOrder)));
        assertEq(abi.decode(arguments[1], (uint256)), 2);
        assertEq(abi.decode(arguments[2], (uint256)), 3);
        assertEq(abi.decode(arguments[3], (uint256)), 0);
        assertEq(attributes.length, 3);
        assertEq(bytes4(attributes[0]), IAttribute.NeedsVariable.selector);
        assertEq(bytes4(attributes[1]), IAttribute.TimingBounds.selector);
        assertEq(bytes4(attributes[2]), IAttribute.RevertPolicy.selector);

        (
            bytes memory paymentToken,
            bytes memory paymentSender,
            bytes memory amountFormula,
            uint256 recipientVariable,
            uint256 stepIndex,
            uint256 delay
        ) = decoder.payment(resolved.payments[0]);
        (uint256 tokenChain, address tokenAddress) = InteroperableAddress.parseEvmV1(paymentToken);
        (uint256 senderChain, address senderAddress) = InteroperableAddress.parseEvmV1(paymentSender);
        assertEq(tokenChain, block.chainid);
        assertEq(tokenAddress, solventOrder.inputToken);
        assertEq(senderChain, block.chainid);
        assertEq(senderAddress, address(filler));
        assertEq(recipientVariable, 0);
        assertEq(stepIndex, 0);
        assertEq(delay, 0);
        assertEq(decoder.constantFormula(amountFormula), solventOrder.executorFee);

        assertEq(bytes4(resolved.variables[0]), IVariableRole.PaymentRecipient.selector);
        assertEq(bytes4(resolved.variables[1]), IVariableRole.StepCaller.selector);
        assertEq(bytes4(resolved.variables[2]), IVariableRole.Witness.selector);
        assertEq(bytes4(resolved.variables[3]), IVariableRole.Witness.selector);

        (string memory permitKind, bytes memory permitData, uint256[] memory permitDependencies) =
            decoder.witness(resolved.variables[2]);
        assertEq(permitKind, resolver.PERMIT_SIGNATURE_WITNESS());
        assertEq(abi.decode(permitData, (bytes32)), settler.orderId(solventOrder));
        assertEq(permitDependencies.length, 0);

        (string memory sourceKind, bytes memory sourceData, uint256[] memory sourceDependencies) =
            decoder.witness(resolved.variables[3]);
        assertEq(sourceKind, resolver.SOURCE_PLAN_WITNESS());
        assertEq(abi.decode(sourceData, (bytes32)), settler.orderId(solventOrder));
        assertEq(sourceDependencies.length, 1);
        assertEq(sourceDependencies[0], 0);
    }

    function test_resolveRejectsAnOrderForAnotherChain() public {
        Solvent7683Order memory solventOrder = _order();
        solventOrder.chainId += 1;

        vm.expectRevert(
            abi.encodeWithSelector(SolventSameChainSettler.WrongChain.selector, solventOrder.chainId, block.chainid)
        );
        resolver.resolve(abi.encode(solventOrder));
    }

    function _order() internal view returns (Solvent7683Order memory) {
        return Solvent7683Order({
            settler: address(settler),
            user: address(0x1111),
            chainId: block.chainid,
            inputToken: address(0x2222),
            inputAmount: 1000,
            outputToken: address(0x3333),
            outputAmount: 900,
            recipient: address(0x4444),
            executorFee: 10,
            nonce: 7,
            deadline: block.timestamp + 1 hours
        });
    }
}
