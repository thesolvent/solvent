// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Ownable2Step } from "@openzeppelin/contracts/access/Ownable2Step.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { ITakerCallbacks } from "@1inch/swap-vm/src/interfaces/ITakerCallbacks.sol";
import { TakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";

import { IDestinationSettler } from "./interfaces/ERC7683.sol";

/// @title Erc7683AquaFiller
/// @notice Fills ERC-7683 orders by sourcing their output from makers' Aqua positions through the SwapVM
///         router, as a pure taker. It holds no inventory and keeps the spread.
/// @dev Unlike UniswapX, ERC-7683's `IDestinationSettler.fill` hands the filler nothing: the output must be
///      delivered before the escrow is released. The flash comes from SwapVM instead. With
///      `isFirstTransferFromTaker = false` the router runs `_transferOut` before `_transferIn`, and
///      `_transferIn` fires `preTransferInCallback` *before* pulling our payment — so inside that callback
///      we hold the maker's output having paid nothing. We call the settler there: it takes the output,
///      pays the user, and releases the escrowed input to us, which then funds the maker.
///
///      Multi-leg sourcing nests: leg i's callback launches leg i+1, the innermost holds the whole output
///      and settles, and the stack unwinds paying makers outward. The remaining plan rides in
///      `preTransferInCallbackData`, so no contract state spans the levels.
contract Erc7683AquaFiller is ITakerCallbacks, Ownable2Step, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    /// @notice One exact-out swap that sources `amountOut` of `tokenOut` from a single maker.
    /// @param router The SwapVM router the maker shipped their Aqua-backed program to.
    /// @param order The maker's order (their program = their price).
    /// @param tokenIn The token we pay the maker.
    /// @param tokenOut The token we source from the maker.
    /// @param amountOut Exact output to source from this maker.
    /// @param amountInMaximum Most input we will pay this maker — the per-leg spread guard.
    struct SourceSwap {
        ISwapVM router;
        ISwapVM.Order order;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInMaximum;
    }

    /// @notice Most makers one fill may source from. Bounds recursion depth and gas; the off-chain
    ///         router rejects wider plans before they are ever submitted.
    uint256 public constant MAX_LEGS = 4;

    /// @dev The router we handed control to, set only for the duration of one fill. Transient (EIP-1153):
    ///      it authenticates the callback within the same tx, never across txs.
    address private transient _routerInFlight;

    event Swept(address indexed token, address indexed to, uint256 amount);

    error CallbackUnauthorized(address caller, address expected);
    error CallbackUnsupported();
    error MixedRouters();
    error NoSources();
    error ProfitabilityGuard(address token, uint256 balanceBefore, uint256 balanceAfter);
    error TooManyLegs(uint256 given, uint256 max);
    error ZeroAddress();

    constructor(address owner_) Ownable(owner_) { }

    /// @notice Fill one ERC-7683 order from the given maker legs. Only the operator submits fills.
    /// @param settler The destination settler the order was opened against.
    /// @param orderId The order's id, as recorded by the settler.
    /// @param originData The order payload the settler expects, verbatim from its `Open` event.
    /// @param sources The off-chain routing plan: the maker legs that source this order's output.
    function fill(
        IDestinationSettler settler,
        bytes32 orderId,
        bytes calldata originData,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        nonReentrant
    {
        require(sources.length != 0, NoSources());
        require(sources.length <= MAX_LEGS, TooManyLegs(sources.length, MAX_LEGS));

        ISwapVM router = sources[0].router;
        for (uint256 i = 1; i < sources.length; ++i) {
            require(address(sources[i].router) == address(router), MixedRouters());
        }

        (address[] memory tokens, uint256[] memory balancesBefore) = _snapshot(sources);

        // Approve the cumulative per-token maximum up front rather than per leg. The legs nest, so an
        // inner leg that re-approved or revoked would strip the allowance the outer leg's own
        // `_transferIn` still needs when the stack unwinds.
        _approveInputs(sources, address(router), true);

        _routerInFlight = address(router);
        _sourceLeg(0, settler, orderId, originData, sources);
        _routerInFlight = address(0);

        _approveInputs(sources, address(router), false);
        _assertNonDecreasing(tokens, balancesBefore);
    }

    /// @inheritdoc ITakerCallbacks
    /// @dev We are holding leg `next - 1`'s output and have paid nothing. Either source the next leg
    ///      (nesting one level deeper) or, holding the whole output, settle.
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
        require(msg.sender == _routerInFlight, CallbackUnauthorized(msg.sender, _routerInFlight));

        (
            IDestinationSettler settler,
            bytes32 orderId,
            bytes memory originData,
            SourceSwap[] memory sources,
            uint256 next
        ) = abi.decode(takerData, (IDestinationSettler, bytes32, bytes, SourceSwap[], uint256));

        if (next < sources.length) {
            _sourceLeg(next, settler, orderId, originData, sources);
        } else {
            _settle(settler, orderId, originData, sources);
        }
    }

    /// @inheritdoc ITakerCallbacks
    /// @dev Never armed — `_takerTraits` sets `hasPreTransferOutCallback` false, so reaching this is a bug.
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

    /// @notice Withdraw accrued spread (or any stray token). The filler is not a vault.
    function sweep(address token, address to) external onlyOwner {
        if (to == address(0)) {
            revert ZeroAddress();
        }
        uint256 amount = IERC20(token).balanceOf(address(this));
        IERC20(token).safeTransfer(to, amount);
        emit Swept(token, to, amount);
    }

    /// @dev Run leg `i`, carrying the rest of the plan in the callback data so no state spans levels.
    function _sourceLeg(
        uint256 i,
        IDestinationSettler settler,
        bytes32 orderId,
        bytes memory originData,
        SourceSwap[] memory sources
    )
        private
    {
        SourceSwap memory s = sources[i];
        bytes memory continuation = abi.encode(settler, orderId, originData, sources, i + 1);
        s.router.swap(s.order, s.tokenIn, s.tokenOut, s.amountOut, _takerTraits(s.amountInMaximum, continuation));
    }

    /// @dev The innermost step: we hold every leg's output. Approve the settler for exactly what we
    ///      sourced — an under-sourcing plan then reverts inside `fill` rather than quietly spending the
    ///      filler's accrued spread.
    function _settle(
        IDestinationSettler settler,
        bytes32 orderId,
        bytes memory originData,
        SourceSwap[] memory sources
    )
        private
    {
        (address[] memory tokens, uint256[] memory amounts) = _sourcedOutputs(sources);
        for (uint256 i; i < tokens.length; ++i) {
            IERC20(tokens[i]).forceApprove(address(settler), amounts[i]);
        }

        settler.fill(orderId, originData, "");

        for (uint256 i; i < tokens.length; ++i) {
            IERC20(tokens[i]).forceApprove(address(settler), 0);
        }
    }

    /// @dev Set or clear the router allowance for every input token, at the plan's cumulative maximum.
    function _approveInputs(SourceSwap[] calldata sources, address router, bool grant) private {
        address[] memory tokens = new address[](sources.length);
        uint256[] memory amounts = new uint256[](sources.length);
        uint256 n;
        for (uint256 i; i < sources.length; ++i) {
            n = _addAmount(tokens, amounts, n, sources[i].tokenIn, sources[i].amountInMaximum);
        }
        for (uint256 i; i < n; ++i) {
            IERC20(tokens[i]).forceApprove(router, grant ? amounts[i] : 0);
        }
    }

    /// @dev Total output sourced per token across the plan.
    function _sourcedOutputs(SourceSwap[] memory sources)
        private
        pure
        returns (address[] memory tokens, uint256[] memory amounts)
    {
        address[] memory scratchTokens = new address[](sources.length);
        uint256[] memory scratchAmounts = new uint256[](sources.length);
        uint256 n;
        for (uint256 i; i < sources.length; ++i) {
            n = _addAmount(scratchTokens, scratchAmounts, n, sources[i].tokenOut, sources[i].amountOut);
        }

        tokens = new address[](n);
        amounts = new uint256[](n);
        for (uint256 i; i < n; ++i) {
            tokens[i] = scratchTokens[i];
            amounts[i] = scratchAmounts[i];
        }
    }

    /// @dev Snapshot this contract's balance of every token the plan touches (`tokenIn` ∪ `tokenOut`).
    function _snapshot(SourceSwap[] calldata sources)
        private
        view
        returns (address[] memory tokens, uint256[] memory balances)
    {
        address[] memory scratch = new address[](sources.length * 2);
        uint256 n;
        for (uint256 i; i < sources.length; ++i) {
            n = _pushUnique(scratch, n, sources[i].tokenIn);
            n = _pushUnique(scratch, n, sources[i].tokenOut);
        }

        tokens = new address[](n);
        balances = new uint256[](n);
        for (uint256 i; i < n; ++i) {
            tokens[i] = scratch[i];
            balances[i] = IERC20(scratch[i]).balanceOf(address(this));
        }
    }

    /// @dev Every touched token must end at least where it started: the fill can neither overpay a maker
    ///      nor dip into the filler's own accrued-spread inventory.
    function _assertNonDecreasing(address[] memory tokens, uint256[] memory balancesBefore) private view {
        for (uint256 i; i < tokens.length; ++i) {
            uint256 balanceAfter = IERC20(tokens[i]).balanceOf(address(this));
            require(balanceAfter >= balancesBefore[i], ProfitabilityGuard(tokens[i], balancesBefore[i], balanceAfter));
        }
    }

    /// @dev Taker config for an exact-out Aqua swap: take the maker's output first, call us back before
    ///      our payment is pulled (the flash), then pull our input via transferFrom + Aqua push.
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

    /// @dev Append `token` to `arr[0..n)` if absent; returns the new length.
    function _pushUnique(address[] memory arr, uint256 n, address token) private pure returns (uint256) {
        for (uint256 i; i < n; ++i) {
            if (arr[i] == token) {
                return n;
            }
        }
        arr[n] = token;
        return n + 1;
    }

    /// @dev Add `amount` to `token`'s running total in the (tokens, amounts) pair; returns the new length.
    function _addAmount(
        address[] memory tokens,
        uint256[] memory amounts,
        uint256 n,
        address token,
        uint256 amount
    )
        private
        pure
        returns (uint256)
    {
        for (uint256 i; i < n; ++i) {
            if (tokens[i] == token) {
                amounts[i] += amount;
                return n;
            }
        }
        tokens[n] = token;
        amounts[n] = amount;
        return n + 1;
    }
}
