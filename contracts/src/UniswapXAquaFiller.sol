// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Ownable2Step } from "@openzeppelin/contracts/access/Ownable2Step.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";

import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { IReactorCallback } from "uniswapx/interfaces/IReactorCallback.sol";
import { ResolvedOrder, SignedOrder, OutputToken } from "uniswapx/base/ReactorStructs.sol";

/// @title UniswapXAquaFiller
/// @notice Fills UniswapX orders by sourcing their outputs from makers' Aqua positions through the SwapVM
///         router, as a pure taker. It holds no inventory and keeps the spread. Routing — which makers
///         fill how much — is decided off-chain and handed in as a `SourceSwap[]` plan; this contract only
///         executes that plan and enforces safety. It re-implements neither pricing nor settlement: the
///         reactor validates orders and moves swapper funds; the router does all Aqua `pull`/`push`.
/// @dev Safety rests on three independent layers:
///      1. per-leg `amountInMaximum`, passed as the SwapVM taker threshold (caps what any maker is paid);
///      2. reactor approvals derived from the resolved orders, so an under-sourcing plan reverts in `_fill`;
///      3. a profitability guard: every token the fill can move ends at least where it started.
///      ERC20 outputs only (native-output orders would revert at the output approval — out of scope for P1).
contract UniswapXAquaFiller is IReactorCallback, Ownable2Step, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;

    /// @notice One exact-out swap that sources `amountOut` of `tokenOut` from a single maker.
    /// @dev Named after Uniswap V3's ExactOutput params — each leg is an exact-output swap.
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

    /// @dev The reactor we handed control to, set only for the duration of one fill. Transient (EIP-1153):
    ///      it authenticates the reactor's callback within the same tx, never across txs.
    address private transient _reactorInFlight;

    /// @notice Emitted when the owner withdraws accrued spread (or any stray token).
    event Swept(address indexed token, address indexed to, uint256 amount);

    error CallbackUnauthorized(address caller, address expected);
    error ProfitabilityGuard(address token, uint256 balanceBefore, uint256 balanceAfter);
    error OutputNotSourced(address token);
    error ZeroAddress();

    constructor(address owner_) Ownable(owner_) { }

    /// @notice Fill one UniswapX order from the given maker legs. Only the operator submits fills.
    /// @param reactor The reactor the swapper's order targets.
    /// @param order The signed UniswapX order.
    /// @param sources The off-chain routing plan: the maker legs that source this order's outputs.
    function fill(
        IReactor reactor,
        SignedOrder calldata order,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        nonReentrant
    {
        (address[] memory tokens, uint256[] memory balancesBefore) = _snapshot(sources);
        _reactorInFlight = address(reactor);
        reactor.executeWithCallback(order, abi.encode(sources));
        _reactorInFlight = address(0);
        _assertNonDecreasing(tokens, balancesBefore);
    }

    /// @notice Fill a batch of UniswapX orders from one shared plan of maker legs.
    /// @param reactor The reactor the orders target.
    /// @param orders The signed UniswapX orders.
    /// @param sources The off-chain routing plan sourcing every output across all orders.
    function fillBatch(
        IReactor reactor,
        SignedOrder[] calldata orders,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        nonReentrant
    {
        (address[] memory tokens, uint256[] memory balancesBefore) = _snapshot(sources);
        _reactorInFlight = address(reactor);
        reactor.executeBatchWithCallback(orders, abi.encode(sources));
        _reactorInFlight = address(0);
        _assertNonDecreasing(tokens, balancesBefore);
    }

    /// @inheritdoc IReactorCallback
    /// @dev The reactor has already moved every swapper's input to this contract. We acquire each order's
    ///      output from the makers, then approve the reactor to collect all outputs.
    function reactorCallback(ResolvedOrder[] memory orders, bytes memory data) external {
        require(msg.sender == _reactorInFlight, CallbackUnauthorized(msg.sender, _reactorInFlight));

        SourceSwap[] memory sources = abi.decode(data, (SourceSwap[]));
        for (uint256 i; i < sources.length; ++i) {
            SourceSwap memory s = sources[i];
            IERC20(s.tokenIn).forceApprove(address(s.router), s.amountInMaximum);
            s.router.swap(s.order, s.tokenIn, s.tokenOut, s.amountOut, _takerTraits(s.amountInMaximum));
            IERC20(s.tokenIn).forceApprove(address(s.router), 0);
        }

        _approveOutputs(orders, sources, msg.sender);
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

    /// @dev Snapshot this contract's balance of every token the plan touches (`tokenIn` ∪ `tokenOut`).
    ///      Because every output must be sourced (see `_approveOutputs`), this set covers every token that
    ///      can leave the filler during the fill.
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

    /// @dev Sum each output token across all orders and approve the reactor once per token — amounts from
    ///      the resolved orders, so approvals always match what the reactor pulls. Requires every output to
    ///      be sourced, which keeps the profitability guard airtight (the token is then in `_snapshot`).
    function _approveOutputs(ResolvedOrder[] memory orders, SourceSwap[] memory sources, address reactor) private {
        uint256 maxLen;
        for (uint256 i; i < orders.length; ++i) {
            maxLen += orders[i].outputs.length;
        }

        address[] memory tokens = new address[](maxLen);
        uint256[] memory amounts = new uint256[](maxLen);
        uint256 n;
        for (uint256 i; i < orders.length; ++i) {
            OutputToken[] memory outputs = orders[i].outputs;
            for (uint256 j; j < outputs.length; ++j) {
                n = _addAmount(tokens, amounts, n, outputs[j].token, outputs[j].amount);
            }
        }

        for (uint256 i; i < n; ++i) {
            if (!_isSourced(sources, tokens[i])) {
                revert OutputNotSourced(tokens[i]);
            }
            IERC20(tokens[i]).forceApprove(reactor, amounts[i]);
        }
    }

    /// @dev Taker config for an exact-out Aqua swap: pull our input via transferFrom + Aqua push, deliver
    ///      the output to us (default recipient), and cap input at `maxInput`.
    function _takerTraits(uint256 maxInput) private view returns (bytes memory) {
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

    /// @dev Whether any leg sources `token` as its output.
    function _isSourced(SourceSwap[] memory sources, address token) private pure returns (bool) {
        for (uint256 i; i < sources.length; ++i) {
            if (sources[i].tokenOut == token) {
                return true;
            }
        }
        return false;
    }
}
