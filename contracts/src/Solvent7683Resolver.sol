// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { InteroperableAddress } from "@openzeppelin/contracts-erc7683/utils/draft-InteroperableAddress.sol";
import { Argument, Attribute, Formula, IResolver, Payment, Step, VariableRole } from "erc7683/ERC7683.sol";

import { IErc7683AquaFiller, ISolventSameChainSettler, Solvent7683Order } from "./interfaces/ISolvent7683.sol";

/// @title Solvent7683Resolver
/// @notice Describes a Solvent same-chain order using the current resolver-based ERC-7683 format.
contract Solvent7683Resolver is IResolver {
    uint256 private constant PAYMENT_RECIPIENT_VARIABLE = 0;
    uint256 private constant STEP_CALLER_VARIABLE = 1;
    uint256 private constant PERMIT_SIGNATURE_VARIABLE = 2;
    uint256 private constant SOURCE_PLAN_VARIABLE = 3;

    string public constant PERMIT_SIGNATURE_WITNESS = "solvent.permit2-signature.v1";
    string public constant SOURCE_PLAN_WITNESS = "solvent.aqua-source-plan.v1";

    ISolventSameChainSettler public immutable SETTLER;
    IErc7683AquaFiller public immutable FILLER;

    error FillerSettlerMismatch(address given, address expected);
    error InvalidContract(address account);
    error InvalidRecipient(address recipient);

    constructor(ISolventSameChainSettler settler_, IErc7683AquaFiller filler_) {
        if (address(settler_).code.length == 0) {
            revert InvalidContract(address(settler_));
        }
        if (address(filler_).code.length == 0) {
            revert InvalidContract(address(filler_));
        }
        address fillerSettler = filler_.settler();
        if (fillerSettler != address(settler_)) {
            revert FillerSettlerMismatch(fillerSettler, address(settler_));
        }
        SETTLER = settler_;
        FILLER = filler_;
    }

    /// @inheritdoc IResolver
    function resolve(bytes calldata payload) external view returns (ResolvedOrder memory order) {
        Solvent7683Order memory solventOrder = abi.decode(payload, (Solvent7683Order));
        bytes32 id = SETTLER.validate(solventOrder);
        if (solventOrder.recipient == address(FILLER)) {
            revert InvalidRecipient(solventOrder.recipient);
        }

        order.variables = _variables(id);
        order.steps = new bytes[](1);
        order.steps[0] = _fillStep(solventOrder);
        order.payments = new bytes[](1);
        order.payments[0] = Payment.ERC20(
            _evm(solventOrder.inputToken),
            _evm(address(FILLER)),
            Formula.Constant(solventOrder.executorFee),
            PAYMENT_RECIPIENT_VARIABLE,
            0,
            0
        );
        order.assumptions = new Assumption[](0);
    }

    function _variables(bytes32 orderId) private pure returns (bytes[] memory variables) {
        variables = new bytes[](4);
        variables[PAYMENT_RECIPIENT_VARIABLE] = VariableRole.PaymentRecipient();
        variables[STEP_CALLER_VARIABLE] = VariableRole.StepCaller(0);
        variables[PERMIT_SIGNATURE_VARIABLE] =
            VariableRole.Witness(PERMIT_SIGNATURE_WITNESS, abi.encode(orderId), new uint256[](0));

        uint256[] memory paymentDependency = new uint256[](1);
        paymentDependency[0] = PAYMENT_RECIPIENT_VARIABLE;
        variables[SOURCE_PLAN_VARIABLE] =
            VariableRole.Witness(SOURCE_PLAN_WITNESS, abi.encode(orderId), paymentDependency);
    }

    function _fillStep(Solvent7683Order memory order) private view returns (bytes memory) {
        bytes[] memory arguments = new bytes[](4);
        arguments[0] = Argument.ConstBytes(abi.encode(order));
        arguments[1] = Argument.Variable(PERMIT_SIGNATURE_VARIABLE);
        arguments[2] = Argument.Variable(SOURCE_PLAN_VARIABLE);
        arguments[3] = Argument.Variable(PAYMENT_RECIPIENT_VARIABLE);

        bytes[] memory attributes = new bytes[](3);
        attributes[0] = Attribute.NeedsVariable(STEP_CALLER_VARIABLE);
        attributes[1] = Attribute.TimingBounds("block.timestamp", "", Formula.Constant(order.deadline));
        attributes[2] = Attribute.RevertPolicy("abort", "");

        return Step.Call(_evm(address(FILLER)), IErc7683AquaFiller.fill.selector, arguments, attributes);
    }

    function _evm(address account) private view returns (bytes memory) {
        return InteroperableAddress.formatEvmV1(block.chainid, account);
    }
}
