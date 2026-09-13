// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Ownable2Step } from "@openzeppelin/contracts/access/Ownable2Step.sol";
import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { ECDSA } from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import { EIP712 } from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import { Pausable } from "@openzeppelin/contracts/utils/Pausable.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { ITakerCallbacks } from "@1inch/swap-vm/src/interfaces/ITakerCallbacks.sol";
import { MakerTraits } from "@1inch/swap-vm/src/libs/MakerTraits.sol";
import { TakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";

import { SolventSameChainSettler } from "./SolventSameChainSettler.sol";
import { SolventTakerCredential } from "./SolventTakerCredential.sol";
import { IErc7683AquaFiller, Solvent7683Order } from "./interfaces/ISolvent7683.sol";

/// @title Erc7683AquaFiller
/// @notice Sources a current ERC-7683 order from policy-authorized protected Aqua strategies and
///         settles the user exchange atomically.
contract Erc7683AquaFiller is
    IErc7683AquaFiller,
    ITakerCallbacks,
    Ownable2Step,
    EIP712,
    Pausable,
    ReentrancyGuardTransient
{
    using SafeERC20 for IERC20;

    string private constant EIP712_NAME = "Solvent Aqua Filler";
    string private constant EIP712_VERSION = "1";

    bytes1 private constant ONLY_TAKER_BALANCE_NON_ZERO_OPCODE = 0x0e;
    bytes1 private constant ADDRESS_ARGUMENT_LENGTH = 0x14;
    uint256 private constant CREDENTIAL_PREFIX_LENGTH = 22;
    uint256 private constant RECEIVER_MASK = type(uint160).max;
    uint256 private constant USE_AQUA_INSTEAD_OF_SIGNATURE_FLAG = uint256(1) << 254;

    uint256 public constant MAX_LEGS = 4;
    bytes32 public constant AUTHORIZATION_TYPEHASH = keccak256(
        "Authorization(uint8 kind,uint256 nonce,bytes32 contextHash,bytes32 strategyHash,address maker,address tokenIn,address tokenOut,uint256 amountOut,uint256 amountInLimit,uint256 rebateAmount,uint64 deadlineBlock)"
    );

    enum ExecutionKind {
        UserFill,
        Rebate
    }

    struct Authorization {
        ExecutionKind kind;
        uint256 nonce;
        bytes32 contextHash;
        bytes32 strategyHash;
        address maker;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInLimit;
        uint256 rebateAmount;
        uint64 deadlineBlock;
    }

    struct SourceSwap {
        ISwapVM.Order order;
        Authorization authorization;
        bytes policySignature;
    }

    ISwapVM public immutable ROUTER;
    SolventSameChainSettler public immutable SETTLER;
    SolventTakerCredential public immutable TAKER_CREDENTIAL;

    address public policySigner;
    mapping(address token => bool allowed) public allowedToken;
    mapping(uint256 word => uint256 bitmap) private _nonceBitmap;

    address private transient _routerInFlight;

    event Filled(
        bytes32 indexed orderId,
        address indexed user,
        address indexed recipient,
        address paymentRecipient,
        address inputToken,
        address outputToken,
        uint256 inputAmount,
        uint256 outputAmount,
        uint256 earned
    );
    event NoncesInvalidated(uint256 indexed word, uint256 mask);
    event PolicySignerUpdated(address indexed previousSigner, address indexed newSigner);
    event Swept(address indexed token, address indexed to, uint256 amount);
    event TokenPermissionUpdated(address indexed token, bool allowed);

    error AssetBalanceDecreased(address token, uint256 balanceBefore, uint256 balanceAfter);
    error AuthorizationContextMismatch(bytes32 supplied, bytes32 expected);
    error AuthorizationExpired(uint64 deadlineBlock, uint256 currentBlock);
    error AuthorizationMakerMismatch(address supplied, address expected);
    error CallbackUnauthorized(address caller, address expected);
    error CallbackUnsupported();
    error CredentialGateMissing();
    error CredentialNotActive();
    error ExecutorFeeNotEarned(uint256 earned, uint256 required);
    error InputLimitExceeded(uint256 supplied, uint256 available);
    error InvalidContract(address account);
    error InvalidPaymentRecipient(address recipient);
    error InvalidExecutionKind(ExecutionKind supplied, ExecutionKind expected);
    error InvalidMakerTraits();
    error InvalidPolicySigner(address recovered, address expected);
    error NonceAlreadyUsed(uint256 nonce);
    error NoSources();
    error OutputAmountMismatch(uint256 supplied, uint256 required);
    error OwnershipRenunciationDisabled();
    error StrategyHashMismatch(bytes32 supplied, bytes32 expected);
    error TokenBalanceMismatch(address token, address account, uint256 expected, uint256 actual);
    error TokenNotAllowed(address token);
    error TokenPairInvalid(address tokenIn, address tokenOut);
    error TooManyLegs(uint256 given, uint256 max);
    error UnexpectedRebate(uint256 amount);
    error ZeroAddress();
    error ZeroAmount();
    error ZeroContext();

    constructor(
        address owner_,
        ISwapVM router_,
        SolventSameChainSettler settler_,
        SolventTakerCredential takerCredential_,
        address policySigner_
    )
        Ownable(owner_)
        EIP712(EIP712_NAME, EIP712_VERSION)
    {
        if (owner_ == address(0) || policySigner_ == address(0)) {
            revert ZeroAddress();
        }
        if (address(router_).code.length == 0) {
            revert InvalidContract(address(router_));
        }
        if (address(settler_).code.length == 0) {
            revert InvalidContract(address(settler_));
        }
        if (address(takerCredential_).code.length == 0) {
            revert InvalidContract(address(takerCredential_));
        }
        ROUTER = router_;
        SETTLER = settler_;
        TAKER_CREDENTIAL = takerCredential_;
        policySigner = policySigner_;
    }

    /// @inheritdoc IErc7683AquaFiller
    function fill(
        bytes calldata orderData,
        bytes calldata permitSignature,
        bytes calldata sourceData,
        address paymentRecipient
    )
        external
        onlyOwner
        whenNotPaused
        nonReentrant
        returns (uint256 earned)
    {
        if (paymentRecipient == address(0) || paymentRecipient == address(this)) {
            revert InvalidPaymentRecipient(paymentRecipient);
        }
        if (TAKER_CREDENTIAL.balanceOf(address(this)) != 1) {
            revert CredentialNotActive();
        }

        Solvent7683Order memory order = abi.decode(orderData, (Solvent7683Order));
        bytes32 id = SETTLER.validate(order);
        SourceSwap[] memory sources = abi.decode(sourceData, (SourceSwap[]));
        if (sources.length == 0) {
            revert NoSources();
        }
        if (sources.length > MAX_LEGS) {
            revert TooManyLegs(sources.length, MAX_LEGS);
        }

        _authorizePlan(order, userFillContext(id, paymentRecipient), sources);

        uint256 inputBefore = IERC20(order.inputToken).balanceOf(address(this));
        uint256 outputBefore = IERC20(order.outputToken).balanceOf(address(this));
        _approveInputs(sources, true);
        _routerInFlight = address(ROUTER);
        _sourceLeg(0, orderData, permitSignature, sources);
        _routerInFlight = address(0);
        _approveInputs(sources, false);

        uint256 inputAfter = IERC20(order.inputToken).balanceOf(address(this));
        if (inputAfter < inputBefore) {
            revert AssetBalanceDecreased(order.inputToken, inputBefore, inputAfter);
        }
        earned = inputAfter - inputBefore;
        if (earned < order.executorFee) {
            revert ExecutorFeeNotEarned(earned, order.executorFee);
        }
        _requireBalance(order.outputToken, address(this), outputBefore);
        _safeTransferExact(order.inputToken, paymentRecipient, earned, inputBefore);

        emit Filled(
            id,
            order.user,
            order.recipient,
            paymentRecipient,
            order.inputToken,
            order.outputToken,
            order.inputAmount,
            order.outputAmount,
            earned
        );
    }

    /// @inheritdoc ITakerCallbacks
    function preTransferInCallback(
        address,
        address,
        address,
        address,
        uint256,
        uint256,
        bytes32,
        bytes calldata takerData
    )
        external
    {
        if (msg.sender != _routerInFlight) {
            revert CallbackUnauthorized(msg.sender, _routerInFlight);
        }

        (bytes memory orderData, bytes memory permitSignature, SourceSwap[] memory sources, uint256 next) =
            abi.decode(takerData, (bytes, bytes, SourceSwap[], uint256));
        if (next < sources.length) {
            _sourceLeg(next, orderData, permitSignature, sources);
            return;
        }

        Solvent7683Order memory order = abi.decode(orderData, (Solvent7683Order));
        IERC20(order.outputToken).forceApprove(address(SETTLER), order.outputAmount);
        SETTLER.settle(order, permitSignature);
        IERC20(order.outputToken).forceApprove(address(SETTLER), 0);
    }

    /// @inheritdoc ITakerCallbacks
    function preTransferOutCallback(
        address,
        address,
        address,
        address,
        uint256,
        uint256,
        bytes32,
        bytes calldata
    )
        external
        pure
    {
        revert CallbackUnsupported();
    }

    function userFillContext(bytes32 orderId, address paymentRecipient) public view returns (bytes32) {
        // Fixed-width ABI fields keep the policy-signing schema explicit across clients.
        // forge-lint: disable-next-line(asm-keccak256)
        return keccak256(abi.encode(address(SETTLER), orderId, paymentRecipient));
    }

    /// @inheritdoc IErc7683AquaFiller
    function settler() external view returns (address) {
        return address(SETTLER);
    }

    function hashAuthorization(Authorization calldata authorization) external view returns (bytes32) {
        return _authorizationDigest(authorization);
    }

    function isNonceUsed(uint256 nonce) public view returns (bool) {
        uint256 mask = uint256(1) << (nonce & 0xff);
        return _nonceBitmap[nonce >> 8] & mask != 0;
    }

    function setPolicySigner(address newSigner) external onlyOwner {
        if (newSigner == address(0)) {
            revert ZeroAddress();
        }
        address previousSigner = policySigner;
        policySigner = newSigner;
        emit PolicySignerUpdated(previousSigner, newSigner);
    }

    function setTokenAllowed(address token, bool allowed) external onlyOwner {
        if (token == address(0)) {
            revert ZeroAddress();
        }
        allowedToken[token] = allowed;
        emit TokenPermissionUpdated(token, allowed);
    }

    function invalidateNonces(uint256 word, uint256 mask) external onlyOwner {
        _nonceBitmap[word] |= mask;
        emit NoncesInvalidated(word, mask);
    }

    function pause() external onlyOwner {
        _pause();
    }

    function unpause() external onlyOwner {
        _unpause();
    }

    function renounceOwnership() public pure override {
        revert OwnershipRenunciationDisabled();
    }

    function sweep(address token, address to) external onlyOwner nonReentrant {
        if (token == address(0) || to == address(0)) {
            revert ZeroAddress();
        }
        uint256 amount = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransfer(to, amount);
        emit Swept(token, to, amount);
    }

    function _authorizePlan(Solvent7683Order memory order, bytes32 contextHash, SourceSwap[] memory sources) private {
        uint256 totalOutput;
        uint256 totalInputLimit;
        for (uint256 i; i < sources.length; ++i) {
            Authorization memory authorization = sources[i].authorization;
            if (authorization.kind != ExecutionKind.UserFill) {
                revert InvalidExecutionKind(authorization.kind, ExecutionKind.UserFill);
            }
            if (authorization.contextHash != contextHash) {
                revert AuthorizationContextMismatch(authorization.contextHash, contextHash);
            }
            if (authorization.rebateAmount != 0) {
                revert UnexpectedRebate(authorization.rebateAmount);
            }
            if (authorization.tokenIn != order.inputToken || authorization.tokenOut != order.outputToken) {
                revert TokenPairInvalid(authorization.tokenIn, authorization.tokenOut);
            }

            _authorize(sources[i].order, authorization, sources[i].policySignature);
            totalOutput += authorization.amountOut;
            totalInputLimit += authorization.amountInLimit;
        }
        if (totalOutput != order.outputAmount) {
            revert OutputAmountMismatch(totalOutput, order.outputAmount);
        }
        uint256 available = order.inputAmount - order.executorFee;
        if (totalInputLimit > available) {
            revert InputLimitExceeded(totalInputLimit, available);
        }
    }

    function _authorize(
        ISwapVM.Order memory makerOrder,
        Authorization memory authorization,
        bytes memory signature
    )
        private
    {
        _validateAuthorizationFields(authorization);
        if (authorization.maker != makerOrder.maker) {
            revert AuthorizationMakerMismatch(authorization.maker, makerOrder.maker);
        }
        bytes32 strategyHash = ROUTER.hash(makerOrder);
        if (strategyHash != authorization.strategyHash) {
            revert StrategyHashMismatch(authorization.strategyHash, strategyHash);
        }
        _validateProtectedStrategy(makerOrder);

        address recovered = ECDSA.recover(_authorizationDigest(authorization), signature);
        if (recovered != policySigner) {
            revert InvalidPolicySigner(recovered, policySigner);
        }
        _consumeNonce(authorization.nonce);
    }

    function _validateAuthorizationFields(Authorization memory authorization) private view {
        if (authorization.deadlineBlock < block.number) {
            revert AuthorizationExpired(authorization.deadlineBlock, block.number);
        }
        if (authorization.contextHash == bytes32(0)) {
            revert ZeroContext();
        }
        if (
            authorization.maker == address(0) || authorization.tokenIn == address(0)
                || authorization.tokenOut == address(0)
        ) {
            revert ZeroAddress();
        }
        if (authorization.amountOut == 0 || authorization.amountInLimit == 0) {
            revert ZeroAmount();
        }
        if (!allowedToken[authorization.tokenIn]) {
            revert TokenNotAllowed(authorization.tokenIn);
        }
        if (!allowedToken[authorization.tokenOut]) {
            revert TokenNotAllowed(authorization.tokenOut);
        }
        if (authorization.tokenIn == authorization.tokenOut) {
            revert TokenPairInvalid(authorization.tokenIn, authorization.tokenOut);
        }
    }

    function _validateProtectedStrategy(ISwapVM.Order memory order) private view {
        uint256 traits = MakerTraits.unwrap(order.traits);
        // The explicit mask makes this narrowing conversion safe.
        // forge-lint: disable-next-line(unsafe-typecast)
        address receiver = address(uint160(traits & RECEIVER_MASK));
        if (
            (traits & ~RECEIVER_MASK) != USE_AQUA_INSTEAD_OF_SIGNATURE_FLAG
                || (receiver != address(0) && receiver != order.maker)
        ) {
            revert InvalidMakerTraits();
        }

        bytes memory program = order.data;
        if (
            program.length < CREDENTIAL_PREFIX_LENGTH || program[0] != ONLY_TAKER_BALANCE_NON_ZERO_OPCODE
                || program[1] != ADDRESS_ARGUMENT_LENGTH
        ) {
            revert CredentialGateMissing();
        }

        address requiredCredential;
        assembly ("memory-safe") {
            requiredCredential := shr(96, mload(add(program, 34)))
        }
        if (requiredCredential != address(TAKER_CREDENTIAL)) {
            revert CredentialGateMissing();
        }
    }

    function _authorizationDigest(Authorization memory authorization) private view returns (bytes32) {
        return _hashTypedDataV4(
            keccak256(
                abi.encode(
                    AUTHORIZATION_TYPEHASH,
                    authorization.kind,
                    authorization.nonce,
                    authorization.contextHash,
                    authorization.strategyHash,
                    authorization.maker,
                    authorization.tokenIn,
                    authorization.tokenOut,
                    authorization.amountOut,
                    authorization.amountInLimit,
                    authorization.rebateAmount,
                    authorization.deadlineBlock
                )
            )
        );
    }

    function _consumeNonce(uint256 nonce) private {
        uint256 word = nonce >> 8;
        uint256 mask = uint256(1) << (nonce & 0xff);
        if (_nonceBitmap[word] & mask != 0) {
            revert NonceAlreadyUsed(nonce);
        }
        _nonceBitmap[word] |= mask;
    }

    function _sourceLeg(
        uint256 index,
        bytes memory orderData,
        bytes memory permitSignature,
        SourceSwap[] memory sources
    )
        private
    {
        SourceSwap memory source = sources[index];
        bytes memory continuation = abi.encode(orderData, permitSignature, sources, index + 1);
        ROUTER.swap(
            source.order,
            source.authorization.tokenIn,
            source.authorization.tokenOut,
            source.authorization.amountOut,
            _takerTraits(source.authorization.amountInLimit, continuation)
        );
    }

    function _approveInputs(SourceSwap[] memory sources, bool grant) private {
        address[] memory tokens = new address[](sources.length);
        uint256[] memory amounts = new uint256[](sources.length);
        uint256 length;
        for (uint256 i; i < sources.length; ++i) {
            length = _accumulateAmount(
                tokens, amounts, length, sources[i].authorization.tokenIn, sources[i].authorization.amountInLimit
            );
        }
        for (uint256 i; i < length; ++i) {
            IERC20(tokens[i]).forceApprove(address(ROUTER), grant ? amounts[i] : 0);
        }
    }

    function _takerTraits(uint256 maxInput, bytes memory callbackData) private view returns (bytes memory) {
        return TakerTraitsLib.build(
            TakerTraitsLib.Args({
                taker: address(this),
                isExactIn: false,
                shouldUnwrapWeth: false,
                isStrictThresholdAmount: false,
                isFirstTransferFromTaker: false,
                useTransferFromAndAquaPush: true,
                threshold: abi.encodePacked(maxInput),
                to: address(0),
                deadline: 0,
                hasPreTransferInCallback: true,
                hasPreTransferOutCallback: false,
                preTransferInHookData: "",
                postTransferInHookData: "",
                preTransferOutHookData: "",
                postTransferOutHookData: "",
                preTransferInCallbackData: callbackData,
                preTransferOutCallbackData: "",
                instructionsArgs: "",
                signature: ""
            })
        );
    }

    function _accumulateAmount(
        address[] memory tokens,
        uint256[] memory amounts,
        uint256 length,
        address token,
        uint256 amount
    )
        private
        pure
        returns (uint256)
    {
        for (uint256 i; i < length; ++i) {
            if (tokens[i] == token) {
                amounts[i] += amount;
                return length;
            }
        }
        tokens[length] = token;
        amounts[length] = amount;
        return length + 1;
    }

    function _safeTransferExact(address token, address recipient, uint256 amount, uint256 remaining) private {
        uint256 balanceBefore = IERC20(token).balanceOf(recipient);
        IERC20(token).safeTransfer(recipient, amount);
        _requireBalance(token, recipient, balanceBefore + amount);
        _requireBalance(token, address(this), remaining);
    }

    function _requireBalance(address token, address account, uint256 expected) private view {
        uint256 actual = IERC20(token).balanceOf(account);
        if (actual != expected) {
            revert TokenBalanceMismatch(token, account, expected, actual);
        }
    }
}
