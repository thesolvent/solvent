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
import { MakerTraits } from "@1inch/swap-vm/src/libs/MakerTraits.sol";
import { TakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";

import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IReactorCallback } from "uniswapx/interfaces/IReactorCallback.sol";
import { OutputToken, ResolvedOrder, SignedOrder } from "uniswapx/base/ReactorStructs.sol";

/// @notice A permanent, nontransferable credential recognized by protected SwapVM strategies.
contract SolventTakerCredential {
    address public immutable HOLDER;

    error ZeroHolder();

    constructor(address holder_) {
        if (holder_ == address(0)) {
            revert ZeroHolder();
        }
        HOLDER = holder_;
    }

    /// @notice Reports the credential only for the filler that deployed this contract.
    /// @dev SwapVM's credential opcode needs only this selector. Omitting ERC-20 transfer and
    ///      approval methods makes the credential impossible to move, delegate, or destroy.
    function balanceOf(address account) external view returns (uint256) {
        return account == HOLDER ? 1 : 0;
    }
}

/// @title UniswapXAquaFiller
/// @notice Executes policy-authorized UniswapX fills and public price-restoring swaps against
///         credential-protected Aqua strategies.
/// @dev Requires EIP-1153 transient storage support.
contract UniswapXAquaFiller is IReactorCallback, Ownable2Step, EIP712, Pausable, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    // --------------------------------------------------------------------------------------------
    // Authorization schema and strategy constraints
    // --------------------------------------------------------------------------------------------

    string private constant EIP712_NAME = "Solvent Aqua Filler";
    string private constant EIP712_VERSION = "1";

    // The public Aqua opcode table assigns onlyTakerTokenBalanceNonZero opcode 14.
    bytes1 private constant ONLY_TAKER_BALANCE_NON_ZERO_OPCODE = 0x0e;
    bytes1 private constant ADDRESS_ARGUMENT_LENGTH = 0x14;
    uint256 private constant CREDENTIAL_PREFIX_LENGTH = 22;

    // The pinned MakerTraits layout reserves the low 160 bits for an optional receiver. Requiring
    // the remaining bits to contain only the Aqua flag rejects hooks, alternate program offsets,
    // zero-input execution, WETH unwrapping, and unknown flags in one fail-closed check.
    uint256 private constant RECEIVER_MASK = type(uint160).max;
    uint256 private constant USE_AQUA_INSTEAD_OF_SIGNATURE_FLAG = uint256(1) << 254;

    bytes32 public constant AUTHORIZATION_TYPEHASH = keccak256(
        "Authorization(uint8 kind,uint256 nonce,bytes32 contextHash,bytes32 strategyHash,address maker,address tokenIn,address tokenOut,uint256 amountOut,uint256 amountInLimit,uint256 rebateAmount,uint64 deadlineBlock)"
    );

    enum ExecutionKind {
        UserFill,
        Rebate
    }

    /// @notice A policy decision authorizing one exact-output SwapVM execution.
    /// @dev The EIP-712 domain binds signatures to this contract and chain. `nonce` prevents
    ///      same-domain replay, while `kind` prevents reuse across user-fill and rebate paths.
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

    /// @notice One policy-authorized exact-output maker leg for a UniswapX callback.
    struct SourceSwap {
        ISwapVM.Order order;
        Authorization authorization;
        bytes policySignature;
    }

    ISwapVM public immutable ROUTER;
    IReactor public immutable REACTOR;
    SolventTakerCredential public immutable TAKER_CREDENTIAL;

    address public policySigner;
    mapping(address token => bool allowed) public allowedToken;
    mapping(uint256 word => uint256 bitmap) private _nonceBitmap;

    /// @dev Authenticates a reactor callback within the transaction that initiated it.
    address private transient _reactorInFlight;

    // --------------------------------------------------------------------------------------------
    // Events and errors
    // --------------------------------------------------------------------------------------------

    event PolicySignerUpdated(address indexed previousSigner, address indexed newSigner);
    event TokenPermissionUpdated(address indexed token, bool allowed);
    event NoncesInvalidated(uint256 indexed word, uint256 mask);
    event RebateExecuted(
        bytes32 indexed contextHash,
        bytes32 indexed strategyHash,
        address indexed executor,
        address maker,
        address tokenIn,
        address tokenOut,
        uint256 amountIn,
        uint256 amountOut,
        uint256 rebateAmount
    );
    event Swept(address indexed token, address indexed to, uint256 amount);

    error AssetBalanceDecreased(address token, uint256 balanceBefore, uint256 balanceAfter);
    error AuthorizationContextMismatch(bytes32 supplied, bytes32 expected);
    error AuthorizationExpired(uint64 deadlineBlock, uint256 currentBlock);
    error AuthorizationMakerMismatch(address supplied, address expected);
    error CallbackUnauthorized(address caller, address expected);
    error CredentialGateMissing();
    error InvalidExecutionKind(ExecutionKind supplied, ExecutionKind expected);
    error InvalidMakerTraits();
    error InvalidPolicySigner(address recovered, address expected);
    error InsufficientSourcedOutput(address token, uint256 required, uint256 sourced);
    error NonceAlreadyUsed(uint256 nonce);
    error OutputNotSourced(address token);
    error OwnershipRenunciationDisabled();
    error RecipientBalanceMismatch(address token, address recipient, uint256 expected, uint256 actual);
    error RebateRequired();
    error StrategyHashMismatch(bytes32 supplied, bytes32 expected);
    error TokenBalanceMismatch(address token, uint256 expected, uint256 actual);
    error TokenNotAllowed(address token);
    error TokenPairInvalid(address tokenIn, address tokenOut);
    error UnexpectedRebate(uint256 amount);
    error ZeroAddress();
    error ZeroAmount();
    error ZeroContext();

    // --------------------------------------------------------------------------------------------
    // Construction
    // --------------------------------------------------------------------------------------------

    /// @param owner_ Operator allowed to submit UniswapX fills and administer policy controls.
    /// @param router_ Immutable SwapVM router used by every maker leg.
    /// @param reactor_ Immutable UniswapX reactor allowed to enter the callback.
    /// @param policySigner_ EOA whose EIP-712 signatures authorize exact maker executions.
    constructor(
        address owner_,
        ISwapVM router_,
        IReactor reactor_,
        address policySigner_
    )
        Ownable(owner_)
        EIP712(EIP712_NAME, EIP712_VERSION)
    {
        if (
            owner_ == address(0) || address(router_) == address(0) || address(reactor_) == address(0)
                || policySigner_ == address(0)
        ) {
            revert ZeroAddress();
        }
        ROUTER = router_;
        REACTOR = reactor_;
        policySigner = policySigner_;
        TAKER_CREDENTIAL = new SolventTakerCredential(address(this));
    }

    // --------------------------------------------------------------------------------------------
    // UniswapX execution
    // --------------------------------------------------------------------------------------------

    /// @notice Fill one UniswapX order using policy-authorized Aqua maker legs.
    function fill(
        SignedOrder calldata order,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        whenNotPaused
        nonReentrant
    {
        (address[] memory tokens, uint256[] memory balancesBefore) = _snapshot(sources);
        bytes32 contextHash = userFillContext(order);

        _reactorInFlight = address(REACTOR);
        REACTOR.executeWithCallback(order, abi.encode(contextHash, sources));
        _reactorInFlight = address(0);
        _clearReactorAllowances(sources);

        _assertBalancesNonDecreasing(tokens, balancesBefore);
    }

    /// @notice Fill a batch of UniswapX orders using one shared authorized maker plan.
    /// @dev Source legs are shared across the batch and intentionally need not match its order count.
    function fillBatch(
        SignedOrder[] calldata orders,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        whenNotPaused
        nonReentrant
    {
        (address[] memory tokens, uint256[] memory balancesBefore) = _snapshot(sources);
        bytes32 contextHash = userFillBatchContext(orders);

        _reactorInFlight = address(REACTOR);
        REACTOR.executeBatchWithCallback(orders, abi.encode(contextHash, sources));
        _reactorInFlight = address(0);
        _clearReactorAllowances(sources);

        _assertBalancesNonDecreasing(tokens, balancesBefore);
    }

    // --------------------------------------------------------------------------------------------
    // Public rebate execution
    // --------------------------------------------------------------------------------------------

    /// @notice Execute an authorized price-restoring swap and pay its rebate directly to the maker.
    /// @dev The caller supplies the exact signed input plus rebate and receives the exact signed output.
    ///      The signature intentionally leaves the executor open so any solver can compete to include
    ///      the restoration; the unordered nonce allows only the first successful execution.
    function executeRebate(
        ISwapVM.Order calldata order,
        Authorization calldata authorization,
        bytes calldata signature
    )
        external
        whenNotPaused
        nonReentrant
    {
        _requireKind(authorization.kind, ExecutionKind.Rebate);
        if (authorization.rebateAmount == 0) {
            revert RebateRequired();
        }

        _authorize(order, authorization, signature);

        uint256 inputBefore = IERC20(authorization.tokenIn).balanceOf(address(this));
        uint256 outputBefore = IERC20(authorization.tokenOut).balanceOf(address(this));
        uint256 makerInputBefore = IERC20(authorization.tokenIn).balanceOf(authorization.maker);
        uint256 executorOutputBefore = IERC20(authorization.tokenOut).balanceOf(msg.sender);
        uint256 deposit = authorization.amountInLimit + authorization.rebateAmount;

        IERC20(authorization.tokenIn).safeTransferFrom(msg.sender, address(this), deposit);
        _requireBalance(authorization.tokenIn, inputBefore + deposit);

        IERC20(authorization.tokenIn).forceApprove(address(ROUTER), authorization.amountInLimit);
        (uint256 amountIn, uint256 amountOut,) = ROUTER.swap(
            order,
            authorization.tokenIn,
            authorization.tokenOut,
            authorization.amountOut,
            _takerTraits(authorization.amountInLimit, true)
        );
        IERC20(authorization.tokenIn).forceApprove(address(ROUTER), 0);

        IERC20(authorization.tokenIn).safeTransfer(authorization.maker, authorization.rebateAmount);
        IERC20(authorization.tokenOut).safeTransfer(msg.sender, authorization.amountOut);

        _requireBalance(authorization.tokenIn, inputBefore);
        _requireBalance(authorization.tokenOut, outputBefore);
        _requireRecipientBalance(
            authorization.tokenIn,
            authorization.maker,
            makerInputBefore + authorization.amountInLimit + authorization.rebateAmount
        );
        _requireRecipientBalance(authorization.tokenOut, msg.sender, executorOutputBefore + authorization.amountOut);

        emit RebateExecuted(
            authorization.contextHash,
            authorization.strategyHash,
            msg.sender,
            authorization.maker,
            authorization.tokenIn,
            authorization.tokenOut,
            amountIn,
            amountOut,
            authorization.rebateAmount
        );
    }

    // --------------------------------------------------------------------------------------------
    // Reactor callback
    // --------------------------------------------------------------------------------------------

    /// @inheritdoc IReactorCallback
    /// @dev This callback deliberately does not take the reentrancy lock: it must re-enter while
    ///      `fill` or `fillBatch` holds that lock. The transient reactor marker limits entry to the
    ///      immutable reactor during the active outer call, and every source still consumes a nonce.
    function reactorCallback(ResolvedOrder[] memory orders, bytes memory data) external {
        if (msg.sender != _reactorInFlight) {
            revert CallbackUnauthorized(msg.sender, _reactorInFlight);
        }

        (bytes32 contextHash, SourceSwap[] memory sources) = abi.decode(data, (bytes32, SourceSwap[]));
        for (uint256 i; i < sources.length; ++i) {
            _executeUserSource(contextHash, sources[i]);
        }

        _approveOutputs(orders, sources, msg.sender);
    }

    // --------------------------------------------------------------------------------------------
    // Authorization views
    // --------------------------------------------------------------------------------------------

    /// @notice Returns the EIP-712 digest the policy signer must sign.
    function hashAuthorization(Authorization calldata authorization) external view returns (bytes32) {
        return _authorizationDigest(authorization);
    }

    /// @notice Binds a maker plan to the complete signed UniswapX order.
    function userFillContext(SignedOrder calldata order) public pure returns (bytes32) {
        return keccak256(abi.encode(order));
    }

    /// @notice Binds a shared maker plan to an ordered batch of signed UniswapX orders.
    function userFillBatchContext(SignedOrder[] calldata orders) public pure returns (bytes32) {
        return keccak256(abi.encode(orders));
    }

    /// @notice Reports whether an authorization nonce has been executed or invalidated.
    function isNonceUsed(uint256 nonce) public view returns (bool) {
        uint256 mask = uint256(1) << (nonce & 0xff);
        return _nonceBitmap[nonce >> 8] & mask != 0;
    }

    // --------------------------------------------------------------------------------------------
    // Policy administration
    // --------------------------------------------------------------------------------------------

    /// @notice Rotates the EOA trusted to authorize maker executions.
    function setPolicySigner(address newSigner) external onlyOwner {
        if (newSigner == address(0)) {
            revert ZeroAddress();
        }
        address previousSigner = policySigner;
        policySigner = newSigner;
        emit PolicySignerUpdated(previousSigner, newSigner);
    }

    /// @notice Restricts execution to tokens whose ERC-20 behavior the operator has reviewed.
    function setTokenAllowed(address token, bool allowed) external onlyOwner {
        if (token == address(0)) {
            revert ZeroAddress();
        }
        allowedToken[token] = allowed;
        emit TokenPermissionUpdated(token, allowed);
    }

    /// @notice Cancels any subset of 256 unordered nonces in one storage word.
    function invalidateNonces(uint256 word, uint256 mask) external onlyOwner {
        _nonceBitmap[word] |= mask;
        emit NoncesInvalidated(word, mask);
    }

    /// @notice Stops new fills and rebate executions while preserving recovery controls.
    function pause() external onlyOwner {
        _pause();
    }

    /// @notice Resumes fills and rebate executions.
    function unpause() external onlyOwner {
        _unpause();
    }

    /// @notice Keeps an operator available for signer rotation, incident response, and recovery.
    function renounceOwnership() public pure override {
        revert OwnershipRenunciationDisabled();
    }

    /// @notice Withdraw accrued spread or a stray token. The filler is not a vault.
    function sweep(address token, address to) external onlyOwner nonReentrant {
        if (token == address(0) || to == address(0)) {
            revert ZeroAddress();
        }
        uint256 amount = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransfer(to, amount);
        emit Swept(token, to, amount);
    }

    // --------------------------------------------------------------------------------------------
    // Authorization validation
    // --------------------------------------------------------------------------------------------

    function _executeUserSource(bytes32 contextHash, SourceSwap memory source) private {
        Authorization memory authorization = source.authorization;
        _requireKind(authorization.kind, ExecutionKind.UserFill);
        if (authorization.contextHash != contextHash) {
            revert AuthorizationContextMismatch(authorization.contextHash, contextHash);
        }
        if (authorization.rebateAmount != 0) {
            revert UnexpectedRebate(authorization.rebateAmount);
        }

        _authorize(source.order, authorization, source.policySignature);

        IERC20(authorization.tokenIn).forceApprove(address(ROUTER), authorization.amountInLimit);
        ROUTER.swap(
            source.order,
            authorization.tokenIn,
            authorization.tokenOut,
            authorization.amountOut,
            _takerTraits(authorization.amountInLimit, false)
        );
        IERC20(authorization.tokenIn).forceApprove(address(ROUTER), 0);
    }

    function _authorize(
        ISwapVM.Order memory order,
        Authorization memory authorization,
        bytes memory signature
    )
        private
    {
        _validateAuthorizationFields(authorization);
        _validateOrderBinding(order, authorization);
        _validateProtectedStrategy(order);
        _validatePolicySignature(authorization, signature);
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

    function _validateOrderBinding(ISwapVM.Order memory order, Authorization memory authorization) private view {
        if (authorization.maker != order.maker) {
            revert AuthorizationMakerMismatch(authorization.maker, order.maker);
        }

        bytes32 strategyHash = ROUTER.hash(order);
        if (strategyHash != authorization.strategyHash) {
            revert StrategyHashMismatch(authorization.strategyHash, strategyHash);
        }
    }

    function _validatePolicySignature(Authorization memory authorization, bytes memory signature) private view {
        address recovered = ECDSA.recover(_authorizationDigest(authorization), signature);
        if (recovered != policySigner) {
            revert InvalidPolicySigner(recovered, policySigner);
        }
    }

    function _validateProtectedStrategy(ISwapVM.Order memory order) private view {
        uint256 traits = MakerTraits.unwrap(order.traits);
        // The explicit mask makes the narrowing conversion safe and documents the packed boundary.
        // forge-lint: disable-next-line(unsafe-typecast)
        address receiver = address(uint160(traits & RECEIVER_MASK));
        if (
            (traits & ~RECEIVER_MASK) != USE_AQUA_INSTEAD_OF_SIGNATURE_FLAG
                || (receiver != address(0) && receiver != order.maker)
        ) {
            revert InvalidMakerTraits();
        }

        _validateCredentialGate(order.data);
    }

    /// @dev The first VM instruction is tied to the opcode table in the pinned SwapVM dependency.
    ///      The integration suite builds this same instruction through SwapVM's ProgramBuilder, so
    ///      an upstream opcode change fails closed instead of silently authorizing a different opcode.
    function _validateCredentialGate(bytes memory program) private view {
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

    // --------------------------------------------------------------------------------------------
    // Fill accounting
    // --------------------------------------------------------------------------------------------

    function _snapshot(SourceSwap[] calldata sources)
        private
        view
        returns (address[] memory tokens, uint256[] memory balances)
    {
        address[] memory scratch = new address[](sources.length * 2);
        uint256 n;
        for (uint256 i; i < sources.length; ++i) {
            n = _pushUnique(scratch, n, sources[i].authorization.tokenIn);
            n = _pushUnique(scratch, n, sources[i].authorization.tokenOut);
        }

        tokens = new address[](n);
        balances = new uint256[](n);
        for (uint256 i; i < n; ++i) {
            tokens[i] = scratch[i];
            balances[i] = IERC20(scratch[i]).balanceOf(address(this));
        }
    }

    function _assertBalancesNonDecreasing(address[] memory tokens, uint256[] memory balancesBefore) private view {
        for (uint256 i; i < tokens.length; ++i) {
            uint256 balanceAfter = IERC20(tokens[i]).balanceOf(address(this));
            if (balanceAfter < balancesBefore[i]) {
                revert AssetBalanceDecreased(tokens[i], balancesBefore[i], balanceAfter);
            }
        }
    }

    function _clearReactorAllowances(SourceSwap[] calldata sources) private {
        for (uint256 i; i < sources.length; ++i) {
            IERC20(sources[i].authorization.tokenOut).forceApprove(address(REACTOR), 0);
        }
    }

    function _approveOutputs(ResolvedOrder[] memory orders, SourceSwap[] memory sources, address reactor_) private {
        uint256 maxLength;
        for (uint256 i; i < orders.length; ++i) {
            maxLength += orders[i].outputs.length;
        }

        address[] memory tokens = new address[](maxLength);
        uint256[] memory amounts = new uint256[](maxLength);
        uint256 n;
        for (uint256 i; i < orders.length; ++i) {
            OutputToken[] memory outputs = orders[i].outputs;
            for (uint256 j; j < outputs.length; ++j) {
                n = _accumulateAmount(tokens, amounts, n, outputs[j].token, outputs[j].amount);
            }
        }

        for (uint256 i; i < n; ++i) {
            uint256 sourced = _sourcedAmount(sources, tokens[i]);
            if (sourced == 0) {
                revert OutputNotSourced(tokens[i]);
            }
            if (sourced < amounts[i]) {
                revert InsufficientSourcedOutput(tokens[i], amounts[i], sourced);
            }
            IERC20(tokens[i]).forceApprove(reactor_, amounts[i]);
        }
    }

    function _takerTraits(uint256 inputThreshold, bool strict) private view returns (bytes memory) {
        return TakerTraitsLib.build(
            TakerTraitsLib.Args({
                taker: address(this),
                isExactIn: false,
                shouldUnwrapWeth: false,
                isStrictThresholdAmount: strict,
                isFirstTransferFromTaker: false,
                useTransferFromAndAquaPush: true,
                threshold: abi.encodePacked(inputThreshold),
                to: address(0),
                deadline: 0,
                hasPreTransferInCallback: false,
                hasPreTransferOutCallback: false,
                preTransferInHookData: "",
                postTransferInHookData: "",
                preTransferOutHookData: "",
                postTransferOutHookData: "",
                preTransferInCallbackData: "",
                preTransferOutCallbackData: "",
                instructionsArgs: "",
                signature: ""
            })
        );
    }

    function _requireKind(ExecutionKind supplied, ExecutionKind expected) private pure {
        if (supplied != expected) {
            revert InvalidExecutionKind(supplied, expected);
        }
    }

    function _requireBalance(address token, uint256 expected) private view {
        uint256 actual = IERC20(token).balanceOf(address(this));
        if (actual != expected) {
            revert TokenBalanceMismatch(token, expected, actual);
        }
    }

    function _requireRecipientBalance(address token, address recipient, uint256 expected) private view {
        uint256 actual = IERC20(token).balanceOf(recipient);
        if (actual != expected) {
            revert RecipientBalanceMismatch(token, recipient, expected, actual);
        }
    }

    function _pushUnique(address[] memory array, uint256 length, address token) private pure returns (uint256) {
        for (uint256 i; i < length; ++i) {
            if (array[i] == token) {
                return length;
            }
        }
        array[length] = token;
        return length + 1;
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

    function _sourcedAmount(SourceSwap[] memory sources, address token) private pure returns (uint256 amount) {
        for (uint256 i; i < sources.length; ++i) {
            if (sources[i].authorization.tokenOut == token) {
                amount += sources[i].authorization.amountOut;
            }
        }
    }
}
