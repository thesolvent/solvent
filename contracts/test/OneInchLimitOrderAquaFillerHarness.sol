// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";
import { WETHMock } from "@1inch/swap-vm/test/mocks/WETHMock.sol";

import { LimitOrderProtocol } from "@1inch/limit-order-protocol-contract/contracts/LimitOrderProtocol.sol";
import { IOrderMixin } from "@1inch/limit-order-protocol-contract/contracts/interfaces/IOrderMixin.sol";
import { Address as LOPAddress } from "@1inch/solidity-utils/contracts/libraries/AddressLib.sol";
import { MakerTraits } from "@1inch/limit-order-protocol-contract/contracts/libraries/MakerTraitsLib.sol";

import { OneInchLimitOrderAquaFiller } from "../src/OneInchLimitOrderAquaFiller.sol";

/// @notice Shared machinery for the `OneInchLimitOrderAquaFiller` suites: 1inch's Aqua/SwapVM maker
///         builders + real 1inch Limit Order Protocol v4 order signing, mirroring
///         `UniswapXAquaFillerHarness`'s shape for the UniswapX filler.
abstract contract OneInchLimitOrderAquaFillerHarness is AquaStrategyBuilders {
    AquaSwapVMRouter internal swapVM;
    LimitOrderProtocol internal protocol;
    OneInchLimitOrderAquaFiller internal filler;

    uint256 internal constant SIGNER_PK = 0x1010;
    address internal signer;

    // `_MAKER_AMOUNT_FLAG` (bit 255): the fill `amount` we pass is a making amount, matching this
    // codebase's exact-out convention (the sourcing side asks for an exact output within a max input).
    uint256 internal constant NO_TRAITS_BASE = 0;
    uint256 internal constant MAKER_AMOUNT_FLAG = 1 << 255;

    constructor() AquaStrategyBuilders(address(aqua)) { }

    /// @dev Deploy the maker side (Aqua router, tokens, maker), a fresh `LimitOrderProtocol`, and the
    ///      filler.
    function _deploy() internal {
        super.setUp(); // sets maker, tokenA, tokenB
        swapVM = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        protocol = new LimitOrderProtocol(WETHMock(payable(address(new WETHMock()))));
        filler = new OneInchLimitOrderAquaFiller(address(this), IOrderMixin(address(protocol)));

        signer = vm.addr(SIGNER_PK);
    }

    /// @dev Ship an XYC Aqua strategy for `mkr`, and mint it `realOut` of the output token so Aqua's
    ///      `pull` can settle. Identical to `UniswapXAquaFillerHarness._shipXycMaker`.
    function _shipXycMaker(
        address mkr,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 reserveIn,
        uint256 reserveOut,
        uint256 realOut
    )
        internal
        returns (ISwapVM.Order memory order)
    {
        maker = mkr;
        order = createStrategy(
            MakerSetup({
                balanceA: 0,
                balanceB: 0,
                priceMin: 0,
                priceMax: 0,
                protocolFeeBps: 0,
                feeInBps: 0,
                protocolFeeRecipient: address(0),
                swapType: SwapType.XYC
            })
        );
        shipStrategy(swapVM, order, tokenIn, tokenOut, reserveIn, reserveOut);
        tokenOut.mint(mkr, realOut);
    }

    function _source(
        ISwapVM.Order memory order,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 amountOut,
        uint256 amountInMaximum
    )
        internal
        view
        returns (OneInchLimitOrderAquaFiller.SourceSwap memory)
    {
        return OneInchLimitOrderAquaFiller.SourceSwap({
            router: ISwapVM(address(swapVM)),
            order: order,
            tokenIn: address(tokenIn),
            tokenOut: address(tokenOut),
            amountOut: amountOut,
            amountInMaximum: amountInMaximum
        });
    }

    /// @dev A plain order (no extension), signed by `signer`, giving `makingAmount` of `makerAsset`
    ///      for `takingAmount` of `takerAsset`. `signer` approves the protocol for `makingAmount`.
    function _signPlainOrder(
        TokenMock makerAsset,
        TokenMock takerAsset,
        uint256 makingAmount,
        uint256 takingAmount
    )
        internal
        returns (IOrderMixin.Order memory order, bytes32 r, bytes32 vs)
    {
        order = IOrderMixin.Order({
            salt: uint256(keccak256(abi.encodePacked(block.timestamp, makingAmount, takingAmount))) >> 96,
            maker: LOPAddress.wrap(uint256(uint160(signer))),
            receiver: LOPAddress.wrap(0),
            makerAsset: LOPAddress.wrap(uint256(uint160(address(makerAsset)))),
            takerAsset: LOPAddress.wrap(uint256(uint160(address(takerAsset)))),
            makingAmount: makingAmount,
            takingAmount: takingAmount,
            makerTraits: MakerTraits.wrap(0)
        });

        bytes32 orderHash = protocol.hashOrder(order);
        (uint8 v, bytes32 sigR, bytes32 s) = vm.sign(SIGNER_PK, orderHash);
        r = sigR;
        // Compact EIP-2098 signature: vs packs the recovery bit into s's top bit.
        vs = bytes32((uint256(v - 27) << 255) | uint256(s));

        makerAsset.mint(signer, makingAmount);
        vm.prank(signer);
        makerAsset.approve(address(protocol), makingAmount);
    }
}
