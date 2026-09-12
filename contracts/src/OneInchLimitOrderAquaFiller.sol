// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { IERC20 } from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import { SafeERC20 } from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Ownable2Step } from "@openzeppelin/contracts/access/Ownable2Step.sol";
import { ReentrancyGuardTransient } from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TakerTraitsLib as SwapVMTakerTraitsLib } from "@1inch/swap-vm/src/libs/TakerTraits.sol";

import { IOrderMixin } from "@1inch/limit-order-protocol-contract/contracts/interfaces/IOrderMixin.sol";
import { ITakerInteraction } from "@1inch/limit-order-protocol-contract/contracts/interfaces/ITakerInteraction.sol";
import { AddressLib, Address as LOPAddress } from "@1inch/solidity-utils/contracts/libraries/AddressLib.sol";
import {
    TakerTraits as LOPTakerTraits
} from "@1inch/limit-order-protocol-contract/contracts/libraries/TakerTraitsLib.sol";

/// @title OneInchLimitOrderAquaFiller
/// @notice Fills 1inch Limit Order Protocol v4 orders by sourcing their taker-asset leg from makers'
///         Aqua positions through the SwapVM router, as a pure taker. It holds no inventory and keeps
///         the spread. Routing — which makers fill how much — is decided off-chain and handed in as a
///         `SourceSwap[]` plan; this contract only executes that plan and enforces safety.
/// @dev Unlike UniswapX's reactor (which calls back into the filler mid-fill), 1inch's protocol is
///      called directly by the taker: this contract IS the taker, and asks the protocol to call back
///      into its own `takerInteraction` (`ITakerInteraction`) between the maker->taker and
///      taker->maker legs of one `fillOrderArgs` call — the protocol's own "flash taker" hook,
///      documented in `ITakerInteraction`: "`preInteraction` => `Transfer maker->taker` =>
///      **`Interaction`** => `Transfer taker->maker` => `postInteraction`". We receive the order's
///      makerAsset first, source the takerAsset for it via Aqua/SwapVM inside that callback, then the
///      protocol pulls the takerAsset back out of us to pay the maker.
/// @dev Safety rests on four independent layers:
///      1. per-leg `amountInMaximum`, passed as the SwapVM taker threshold (caps what any maker is paid);
///      2. an approval to the protocol for exactly `takingAmount` (the fill's actual amount, not the
///         order's full stated amount — an under-sourced leg simply fails that transfer, reverting the
///         whole fill, since the protocol pulls `takerAsset` from us right after our callback returns);
///      3. a profitability guard: every token the fill can move ends at least where it started;
///      4. `_activeOrderHash`, the hash of the one order `fill()` is currently mid-call on, checked
///         against `takerInteraction`'s own `orderHash` argument. A same-tx flag alone is not enough:
///         `order.makerAsset`/`takerAsset` are maker-chosen and unvalidated, so a malicious token can
///         run arbitrary code from inside the protocol's transfer to us, before our callback runs —
///         including calling `protocol.fillOrderArgs` on an attacker's own second order that also
///         names us as its interaction target. `msg.sender` is still the protocol either way, and a
///         bare "some fill is in progress" flag would still be set, so the check has to be bound to
///         which order, not just whether one is in flight.
///      ERC20 assets only (this contract never holds ETH, and 1inch's ETH-unwrap flag is left unset).
contract OneInchLimitOrderAquaFiller is ITakerInteraction, Ownable2Step, ReentrancyGuardTransient {
    using SafeERC20 for IERC20;
    using AddressLib for LOPAddress;

    /// @notice One exact-out swap that sources `amountOut` of `tokenOut` from a single maker.
    /// @dev Named after Uniswap V3's ExactOutput params — each leg is an exact-output swap. Same
    ///      shape as `UniswapXAquaFiller.SourceSwap`.
    struct SourceSwap {
        ISwapVM router;
        ISwapVM.Order order;
        address tokenIn;
        address tokenOut;
        uint256 amountOut;
        uint256 amountInMaximum;
    }

    /// @notice The deployed 1inch Limit Order Protocol / Aggregation Router V6 this filler targets.
    IOrderMixin public immutable protocol;

    /// @dev Bits this contract owns in the `takerTraits` it builds — `ARGS_HAS_TARGET` (251) and the
    ///      `ARGS_EXTENSION_LENGTH`/`ARGS_INTERACTION_LENGTH` fields (224-247, 200-223) — per
    ///      `TakerTraitsLib.sol`'s documented layout. The caller-supplied base must leave these zero;
    ///      the four flag bits above them (threshold mode, WETH unwrap, permit skip, Permit2) stay
    ///      caller-controlled.
    uint256 private constant STRUCTURAL_TRAITS_MASK =
        (uint256(1) << 251) | (uint256(0xffffff) << 224) | (uint256(0xffffff) << 200);

    /// @dev Sourcing only ever runs inside a `fill()` we triggered — `takerInteraction` is otherwise
    ///      reachable by anyone through their own unrelated order on the real protocol.
    bool private transient _fillInFlight;
    /// @dev The specific order `fill()` is mid-call on, so a nested callback for a different order
    ///      (reachable via a malicious `makerAsset`'s transfer hook — see the contract-level note)
    ///      cannot pass as this fill's own.
    bytes32 private transient _activeOrderHash;

    /// @notice Emitted when the owner withdraws accrued spread (or any stray token).
    event Swept(address indexed token, address indexed to, uint256 amount);

    error CallbackUnauthorized(address caller);
    error StructuralTraitsBitsSet();
    error ProfitabilityGuard(address token, uint256 balanceBefore, uint256 balanceAfter);
    error ZeroAddress();

    constructor(address owner_, IOrderMixin protocol_) Ownable(owner_) {
        protocol = protocol_;
    }

    /// @notice Fill one 1inch order from the given maker legs.
    /// @param order The order to fill, exactly as the maker signed it.
    /// @param r R component of the maker's signature.
    /// @param vs VS component of the maker's signature.
    /// @param amount The taker-specified fill amount (making or taking, per `takerTraitsBase`'s
    ///        `_MAKER_AMOUNT_FLAG`) — the order's full amount for a non-partial fill.
    /// @param takerTraitsBase Caller-chosen flag bits (threshold amount, `_MAKER_AMOUNT_FLAG`,
    ///        `_SKIP_ORDER_PERMIT_FLAG`) with every structural bit this contract owns left at zero.
    /// @param extension The order's own extension bytes, replayed verbatim (empty if it has none) —
    ///        this codebase only ever fills orders whose extension is a plain 1inch `FeeTaker`
    ///        post-interaction, validated upstream by `OneInchNormalizer`.
    /// @param sources The off-chain routing plan: the maker legs that source the taker-asset leg.
    function fill(
        IOrderMixin.Order calldata order,
        bytes32 r,
        bytes32 vs,
        uint256 amount,
        uint256 takerTraitsBase,
        bytes calldata extension,
        SourceSwap[] calldata sources
    )
        external
        onlyOwner
        nonReentrant
    {
        require(takerTraitsBase & STRUCTURAL_TRAITS_MASK == 0, StructuralTraitsBitsSet());

        (address[] memory tokens, uint256[] memory balancesBefore) =
            _snapshot(order.makerAsset.get(), order.takerAsset.get(), sources);

        bytes memory interaction = abi.encodePacked(address(this), abi.encode(sources));
        bytes memory args = abi.encodePacked(extension, interaction);
        LOPTakerTraits traits =
            LOPTakerTraits.wrap(takerTraitsBase | (extension.length << 224) | (interaction.length << 200));

        _fillInFlight = true;
        _activeOrderHash = protocol.hashOrder(order);
        protocol.fillOrderArgs(order, r, vs, amount, traits, args);
        _fillInFlight = false;
        _activeOrderHash = bytes32(0);

        _assertNonDecreasing(tokens, balancesBefore);
    }

    /// @inheritdoc ITakerInteraction
    /// @dev Called by the protocol after it has sent us `makingAmount` of the order's makerAsset, and
    ///      before it pulls `takingAmount` of the takerAsset back out of us. We source the latter from
    ///      the former via Aqua/SwapVM and leave the protocol an allowance to collect it.
    function takerInteraction(
        IOrderMixin.Order calldata order,
        bytes calldata, /* extension */
        bytes32 orderHash,
        address, /* taker */
        uint256, /* makingAmount */
        uint256 takingAmount,
        uint256, /* remainingMakingAmount */
        bytes calldata extraData
    )
        external
    {
        require(
            msg.sender == address(protocol) && _fillInFlight && orderHash == _activeOrderHash,
            CallbackUnauthorized(msg.sender)
        );

        SourceSwap[] memory sources = abi.decode(extraData, (SourceSwap[]));
        for (uint256 i; i < sources.length; ++i) {
            SourceSwap memory s = sources[i];
            IERC20(s.tokenIn).forceApprove(address(s.router), s.amountInMaximum);
            s.router.swap(s.order, s.tokenIn, s.tokenOut, s.amountOut, _swapVmTakerTraits(s.amountInMaximum));
            IERC20(s.tokenIn).forceApprove(address(s.router), 0);
        }

        // The protocol pulls exactly `takingAmount` next; an under-sourced leg reverts that transfer,
        // taking the whole fill down with it rather than leaving anything partially settled.
        IERC20(order.takerAsset.get()).forceApprove(address(protocol), takingAmount);
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

    /// @dev Snapshot this contract's balance of every token the fill touches: the order's own two
    ///      legs, plus every source leg's `tokenIn`/`tokenOut`.
    function _snapshot(
        address makerAsset,
        address takerAsset,
        SourceSwap[] calldata sources
    )
        private
        view
        returns (address[] memory tokens, uint256[] memory balances)
    {
        address[] memory scratch = new address[](sources.length * 2 + 2);
        uint256 n = _pushUnique(scratch, 0, makerAsset);
        n = _pushUnique(scratch, n, takerAsset);
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

    /// @dev Every touched token must end at least where it started: the fill can neither overpay a
    ///      maker nor dip into the filler's own accrued-spread inventory.
    function _assertNonDecreasing(address[] memory tokens, uint256[] memory balancesBefore) private view {
        for (uint256 i; i < tokens.length; ++i) {
            uint256 balanceAfter = IERC20(tokens[i]).balanceOf(address(this));
            require(balanceAfter >= balancesBefore[i], ProfitabilityGuard(tokens[i], balancesBefore[i], balanceAfter));
        }
    }

    /// @dev Taker config for an exact-out Aqua swap: pull our input via transferFrom + Aqua push,
    ///      deliver the output to us (default recipient), and cap input at `maxInput`. Identical to
    ///      `UniswapXAquaFiller._takerTraits` — a different `TakerTraits` type (SwapVM's, for the
    ///      maker leg), unrelated to 1inch's own `TakerTraits` used in `fill` above.
    function _swapVmTakerTraits(uint256 maxInput) private view returns (bytes memory) {
        return SwapVMTakerTraitsLib.build(
            SwapVMTakerTraitsLib.Args({
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
}
