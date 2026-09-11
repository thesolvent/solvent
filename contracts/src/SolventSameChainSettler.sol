// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { SolventTakerCredential } from "./SolventTakerCredential.sol";
import { ISolventSameChainSettler, Solvent7683Order } from "./interfaces/ISolvent7683.sol";

/// @title SolventSameChainSettler
/// @notice Atomically exchanges a user's Permit2-authorized input for output already sourced by a
///         protected Solvent filler.
contract SolventSameChainSettler is ISolventSameChainSettler, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    string public constant ORDER_TYPE = "Solvent7683Order(address settler,address user,uint256 chainId,"
        "address inputToken,uint256 inputAmount,address outputToken,uint256 outputAmount,address recipient,"
        "uint256 executorFee,uint256 nonce,uint256 deadline)";
    string public constant WITNESS_TYPE_STRING = "Solvent7683Order witness)"
        "Solvent7683Order(address settler,address user,uint256 chainId,address inputToken,uint256 inputAmount,"
        "address outputToken,uint256 outputAmount,address recipient,uint256 executorFee,uint256 nonce,"
        "uint256 deadline)TokenPermissions(address token,uint256 amount)";

    bytes32 public constant ORDER_TYPEHASH = keccak256(bytes(ORDER_TYPE));

    ISignatureTransfer public immutable PERMIT2;
    SolventTakerCredential public immutable TAKER_CREDENTIAL;

    event Settled(
        bytes32 indexed orderId,
        address indexed filler,
        address indexed user,
        address recipient,
        address inputToken,
        address outputToken,
        uint256 inputAmount,
        uint256 outputAmount
    );

    error CredentialNotActive(address caller);
    error DeadlinePassed(uint256 deadline, uint256 currentTimestamp);
    error ExecutorFeeInvalid(uint256 fee, uint256 inputAmount);
    error InvalidContract(address account);
    error InvalidRecipient(address recipient);
    error OutputBalanceInsufficient(uint256 available, uint256 required);
    error TokenBalanceMismatch(address token, address account, uint256 expected, uint256 actual);
    error TokenPairInvalid(address inputToken, address outputToken);
    error WrongChain(uint256 given, uint256 expected);
    error WrongSettler(address given, address expected);
    error ZeroAddress();
    error ZeroAmount();

    constructor(ISignatureTransfer permit2_, SolventTakerCredential takerCredential_) {
        if (address(permit2_).code.length == 0) {
            revert InvalidContract(address(permit2_));
        }
        if (address(takerCredential_).code.length == 0) {
            revert InvalidContract(address(takerCredential_));
        }
        PERMIT2 = permit2_;
        TAKER_CREDENTIAL = takerCredential_;
    }

    /// @inheritdoc ISolventSameChainSettler
    function validate(Solvent7683Order calldata order) public view returns (bytes32) {
        if (order.settler != address(this)) {
            revert WrongSettler(order.settler, address(this));
        }
        if (order.chainId != block.chainid) {
            revert WrongChain(order.chainId, block.chainid);
        }
        if (
            order.user == address(0) || order.inputToken == address(0) || order.outputToken == address(0)
                || order.recipient == address(0)
        ) {
            revert ZeroAddress();
        }
        if (order.inputToken == order.outputToken) {
            revert TokenPairInvalid(order.inputToken, order.outputToken);
        }
        if (order.inputAmount == 0 || order.outputAmount == 0) {
            revert ZeroAmount();
        }
        if (order.executorFee == 0 || order.executorFee >= order.inputAmount) {
            revert ExecutorFeeInvalid(order.executorFee, order.inputAmount);
        }
        if (order.deadline < block.timestamp) {
            revert DeadlinePassed(order.deadline, block.timestamp);
        }
        return orderId(order);
    }

    /// @inheritdoc ISolventSameChainSettler
    function orderId(Solvent7683Order calldata order) public pure returns (bytes32) {
        // Permit2 signers reproduce this canonical ABI encoding offchain.
        bytes memory encoded = abi.encode(
            ORDER_TYPEHASH,
            order.settler,
            order.user,
            order.chainId,
            order.inputToken,
            order.inputAmount,
            order.outputToken,
            order.outputAmount,
            order.recipient,
            order.executorFee,
            order.nonce,
            order.deadline
        );
        // forge-lint: disable-next-line(asm-keccak256)
        return keccak256(encoded);
    }

    /// @inheritdoc ISolventSameChainSettler
    function settle(Solvent7683Order calldata order, bytes calldata permitSignature) external nonReentrant {
        bytes32 id = validate(order);
        if (TAKER_CREDENTIAL.balanceOf(msg.sender) != 1) {
            revert CredentialNotActive(msg.sender);
        }
        if (order.recipient == msg.sender) {
            revert InvalidRecipient(order.recipient);
        }

        uint256 fillerInputBefore = IERC20(order.inputToken).balanceOf(msg.sender);
        PERMIT2.permitWitnessTransferFrom(
            ISignatureTransfer.PermitTransferFrom({
                permitted: ISignatureTransfer.TokenPermissions({ token: order.inputToken, amount: order.inputAmount }),
                nonce: order.nonce,
                deadline: order.deadline
            }),
            ISignatureTransfer.SignatureTransferDetails({ to: msg.sender, requestedAmount: order.inputAmount }),
            order.user,
            id,
            WITNESS_TYPE_STRING,
            permitSignature
        );
        _requireBalance(order.inputToken, msg.sender, fillerInputBefore + order.inputAmount);

        uint256 fillerOutputBefore = IERC20(order.outputToken).balanceOf(msg.sender);
        if (fillerOutputBefore < order.outputAmount) {
            revert OutputBalanceInsufficient(fillerOutputBefore, order.outputAmount);
        }
        uint256 recipientOutputBefore = IERC20(order.outputToken).balanceOf(order.recipient);
        IERC20(order.outputToken).safeTransferFrom(msg.sender, order.recipient, order.outputAmount);
        _requireBalance(order.outputToken, msg.sender, fillerOutputBefore - order.outputAmount);
        _requireBalance(order.outputToken, order.recipient, recipientOutputBefore + order.outputAmount);

        emit Settled(
            id,
            msg.sender,
            order.user,
            order.recipient,
            order.inputToken,
            order.outputToken,
            order.inputAmount,
            order.outputAmount
        );
    }

    function _requireBalance(address token, address account, uint256 expected) private view {
        uint256 actual = IERC20(token).balanceOf(account);
        if (actual != expected) {
            revert TokenBalanceMismatch(token, account, expected, actual);
        }
    }
}
