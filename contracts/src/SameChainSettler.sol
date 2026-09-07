// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import {
    FillInstruction,
    GaslessCrossChainOrder,
    IDestinationSettler,
    IOriginSettler,
    OnchainCrossChainOrder,
    Output,
    ResolvedCrossChainOrder
} from "./interfaces/ERC7683.sol";

/// @title SameChainSettler
/// @notice An ERC-7683 v1 settler for orders whose origin and destination are the same chain. It is both
///         the origin and the destination settler, because on one chain they are the same contract, and it
///         grants no filler anything it does not grant every other filler.
/// @dev Same-chain settlement needs no oracle: proof-of-fill *is* the fill, so `fill` releases the escrow
///      to the filler in the same call that takes their output. That is the only liberty taken with the
///      standard, and the standard explicitly leaves settlement to the implementer.
///
///      The escrow is opened through Permit2 `permitWitnessTransferFrom`, which in one call verifies the
///      user's EIP-712 signature, enforces `openDeadline`, consumes a replay-protecting nonce, and moves
///      the input. There is therefore no separate signature check and no replay bookkeeping here.
contract SameChainSettler is IOriginSettler, IDestinationSettler, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    /// @notice The order type this settler serves: a fixed-price swap with an optional exclusivity window.
    struct SolventOrder {
        address inputToken;
        uint256 inputAmount;
        address outputToken;
        uint256 outputAmount;
        address recipient;
        /// @dev `address(0)` for an open order.
        address exclusiveFiller;
        uint32 exclusivityEnds;
    }

    /// @notice What an opened order holds until it is filled.
    struct Escrow {
        address inputToken;
        uint32 fillDeadline;
        bool filled;
        uint256 inputAmount;
        /// @dev Binds the `originData` a filler passes to `fill` to the order that was opened.
        bytes32 orderDataHash;
    }

    // EIP-712 type strings. Their exact bytes drive `orderId` and the Permit2 witness, and are mirrored
    // by the Rust codec — a reordered or renamed field silently breaks every signature, so they are
    // pinned on both sides.
    string internal constant SOLVENT_ORDER_TYPE = "SolventOrder(address inputToken,uint256 inputAmount,"
        "address outputToken,uint256 outputAmount,address recipient,address exclusiveFiller," "uint32 exclusivityEnds)";
    string internal constant GASLESS_ORDER_TYPE = "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,uint256 originChainId,"
        "uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,bytes orderData)";
    /// @dev Permit2 concatenates this onto its own stub; referenced types follow the witness in
    ///      alphabetical order (GaslessCrossChainOrder before TokenPermissions), per EIP-712.
    string internal constant WITNESS_TYPE_STRING = "GaslessCrossChainOrder witness)"
        "GaslessCrossChainOrder(address originSettler,address user,uint256 nonce,uint256 originChainId,"
        "uint32 openDeadline,uint32 fillDeadline,bytes32 orderDataType,bytes orderData)"
        "TokenPermissions(address token,uint256 amount)";

    /// @notice The `orderDataType` this settler accepts.
    bytes32 public constant SOLVENT_ORDER_TYPE_HASH = keccak256(bytes(SOLVENT_ORDER_TYPE));
    bytes32 internal constant GASLESS_ORDER_TYPE_HASH = keccak256(bytes(GASLESS_ORDER_TYPE));

    ISignatureTransfer public immutable PERMIT2;

    mapping(bytes32 orderId => Escrow) public escrows;
    /// @dev Distinguishes repeated `open` calls from one user, which carry no nonce of their own.
    mapping(address user => uint256 nonce) public onchainNonces;

    event Filled(bytes32 indexed orderId, address indexed filler);

    error AlreadyFilled(bytes32 orderId);
    error FillDeadlinePassed(bytes32 orderId);
    error NotExclusiveFiller(address caller, address exclusive);
    error OriginDataMismatch(bytes32 orderId);
    error UnknownOrder(bytes32 orderId);
    error UnsupportedOrderType(bytes32 orderDataType);
    error WrongChain(uint256 given, uint256 expected);
    error WrongSettler(address given, address expected);

    constructor(ISignatureTransfer permit2) {
        PERMIT2 = permit2;
    }

    /// @inheritdoc IOriginSettler
    /// @dev The user signed offchain; anyone may submit. Permit2 both verifies that signature and moves
    ///      the input into escrow.
    function openFor(
        GaslessCrossChainOrder calldata order,
        bytes calldata signature,
        bytes calldata originFillerData
    )
        external
        nonReentrant
    {
        SolventOrder memory inner = _validated(order);
        bytes32 orderId = _orderId(order);
        _escrow(orderId, order, inner);

        PERMIT2.permitWitnessTransferFrom(
            ISignatureTransfer.PermitTransferFrom({
                permitted: ISignatureTransfer.TokenPermissions({ token: inner.inputToken, amount: inner.inputAmount }),
                nonce: order.nonce,
                deadline: order.openDeadline
            }),
            ISignatureTransfer.SignatureTransferDetails({ to: address(this), requestedAmount: inner.inputAmount }),
            order.user,
            orderId,
            WITNESS_TYPE_STRING,
            signature
        );

        emit Open(orderId, _resolve(order, inner, orderId));
        originFillerData; // spec-mandated parameter; this settler runs no origin-side filler selection
    }

    /// @inheritdoc IOriginSettler
    /// @dev The self-signed path: the user is `msg.sender`, so a plain `transferFrom` replaces Permit2.
    function open(OnchainCrossChainOrder calldata order) external nonReentrant {
        GaslessCrossChainOrder memory gasless = _asGasless(order, msg.sender, onchainNonces[msg.sender]++);
        SolventOrder memory inner = _validated(gasless);
        bytes32 orderId = _orderId(gasless);
        _escrow(orderId, gasless, inner);

        IERC20(inner.inputToken).safeTransferFrom(msg.sender, address(this), inner.inputAmount);
        emit Open(orderId, _resolve(gasless, inner, orderId));
    }

    /// @inheritdoc IDestinationSettler
    /// @dev Takes the output from the filler, pays the user, and — this being one chain — releases the
    ///      escrowed input to the filler in the same call. `filled` is set before either transfer.
    function fill(bytes32 orderId, bytes calldata originData, bytes calldata) external nonReentrant {
        Escrow storage escrow = escrows[orderId];
        require(escrow.inputAmount != 0, UnknownOrder(orderId));
        require(!escrow.filled, AlreadyFilled(orderId));
        require(block.timestamp <= escrow.fillDeadline, FillDeadlinePassed(orderId));
        require(keccak256(originData) == escrow.orderDataHash, OriginDataMismatch(orderId));

        SolventOrder memory inner = abi.decode(originData, (SolventOrder));
        if (inner.exclusiveFiller != address(0) && block.timestamp < inner.exclusivityEnds) {
            require(msg.sender == inner.exclusiveFiller, NotExclusiveFiller(msg.sender, inner.exclusiveFiller));
        }

        escrow.filled = true;
        IERC20(inner.outputToken).safeTransferFrom(msg.sender, inner.recipient, inner.outputAmount);
        IERC20(escrow.inputToken).safeTransfer(msg.sender, escrow.inputAmount);
        emit Filled(orderId, msg.sender);
    }

    /// @inheritdoc IOriginSettler
    function resolveFor(
        GaslessCrossChainOrder calldata order,
        bytes calldata
    )
        external
        view
        returns (ResolvedCrossChainOrder memory)
    {
        return _resolve(order, _validated(order), _orderId(order));
    }

    /// @inheritdoc IOriginSettler
    function resolve(OnchainCrossChainOrder calldata order) external view returns (ResolvedCrossChainOrder memory) {
        GaslessCrossChainOrder memory gasless = _asGasless(order, msg.sender, onchainNonces[msg.sender]);
        return _resolve(gasless, _validated(gasless), _orderId(gasless));
    }

    /// @notice The order id for a gasless order — the envelope's EIP-712 struct hash. Named after the
    ///         standard's own `resolveFor`; an integrator needs it to look the order up in `escrows`.
    function orderIdFor(GaslessCrossChainOrder calldata order) external pure returns (bytes32) {
        return _orderId(order);
    }

    /// @dev Decode `orderData` after checking the envelope is one this settler can serve.
    function _validated(GaslessCrossChainOrder memory order) private view returns (SolventOrder memory) {
        require(order.originSettler == address(this), WrongSettler(order.originSettler, address(this)));
        require(order.originChainId == block.chainid, WrongChain(order.originChainId, block.chainid));
        require(order.orderDataType == SOLVENT_ORDER_TYPE_HASH, UnsupportedOrderType(order.orderDataType));
        return abi.decode(order.orderData, (SolventOrder));
    }

    function _escrow(bytes32 id, GaslessCrossChainOrder memory order, SolventOrder memory inner) private {
        escrows[id] = Escrow({
            inputToken: inner.inputToken,
            fillDeadline: order.fillDeadline,
            filled: false,
            inputAmount: inner.inputAmount,
            orderDataHash: keccak256(order.orderData)
        });
    }

    /// @dev An onchain order as its gasless equivalent, so id, validation and resolution are shared.
    function _asGasless(
        OnchainCrossChainOrder calldata order,
        address user,
        uint256 nonce
    )
        private
        view
        returns (GaslessCrossChainOrder memory)
    {
        return GaslessCrossChainOrder({
            originSettler: address(this),
            user: user,
            nonce: nonce,
            originChainId: block.chainid,
            openDeadline: uint32(block.timestamp),
            fillDeadline: order.fillDeadline,
            orderDataType: order.orderDataType,
            orderData: order.orderData
        });
    }

    function _orderId(GaslessCrossChainOrder memory order) private pure returns (bytes32) {
        return keccak256(
            abi.encode(
                GASLESS_ORDER_TYPE_HASH,
                order.originSettler,
                order.user,
                order.nonce,
                order.originChainId,
                order.openDeadline,
                order.fillDeadline,
                order.orderDataType,
                keccak256(order.orderData)
            )
        );
    }

    /// @dev `maxSpent` is what the filler must deliver, `minReceived` what it collects; both are its
    ///      worst case, so a filler can price the order without decoding `orderData`.
    function _resolve(
        GaslessCrossChainOrder memory order,
        SolventOrder memory inner,
        bytes32 id
    )
        private
        view
        returns (ResolvedCrossChainOrder memory)
    {
        Output[] memory maxSpent = new Output[](1);
        maxSpent[0] = Output({
            token: _toBytes32(inner.outputToken),
            amount: inner.outputAmount,
            recipient: _toBytes32(inner.recipient),
            chainId: block.chainid
        });

        Output[] memory minReceived = new Output[](1);
        minReceived[0] = Output({
            token: _toBytes32(inner.inputToken),
            amount: inner.inputAmount,
            recipient: bytes32(0),
            chainId: block.chainid
        });

        FillInstruction[] memory fillInstructions = new FillInstruction[](1);
        fillInstructions[0] = FillInstruction({
            destinationChainId: uint64(block.chainid),
            destinationSettler: _toBytes32(address(this)),
            originData: order.orderData
        });

        return ResolvedCrossChainOrder({
            user: order.user,
            originChainId: order.originChainId,
            openDeadline: order.openDeadline,
            fillDeadline: order.fillDeadline,
            orderId: id,
            maxSpent: maxSpent,
            minReceived: minReceived,
            fillInstructions: fillInstructions
        });
    }

    function _toBytes32(address addr) private pure returns (bytes32) {
        return bytes32(uint256(uint160(addr)));
    }
}
