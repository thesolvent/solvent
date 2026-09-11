// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { Controls, ControlsArgsBuilder } from "@1inch/swap-vm/src/instructions/Controls.sol";
import { TokenMock } from "@1inch/solidity-utils/contracts/mocks/TokenMock.sol";
import { Program, ProgramBuilder } from "@1inch/swap-vm/test/utils/ProgramBuilder.sol";

import { PermitSignature } from "uniswapx-test/util/PermitSignature.sol";
import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { OrderInfoBuilder } from "uniswapx-test/util/OrderInfoBuilder.sol";
import { V2DutchOrderReactor } from "uniswapx/reactors/V2DutchOrderReactor.sol";
import { IReactor } from "uniswapx/interfaces/IReactor.sol";
import { V2DutchOrder, V2DutchOrderLib, CosignerData } from "uniswapx/lib/V2DutchOrderLib.sol";
import { DutchInput, DutchOutput } from "uniswapx/lib/DutchOrderLib.sol";
import { OrderInfo, SignedOrder } from "uniswapx/base/ReactorStructs.sol";
import { IPermit2 } from "permit2/src/interfaces/IPermit2.sol";
import { ERC20 as SolERC20 } from "solmate/src/tokens/ERC20.sol";

import { UniswapXAquaFiller } from "../src/UniswapXAquaFiller.sol";

/// @notice Shared machinery for the `UniswapXAquaFiller` suites: 1inch's Aqua/SwapVM maker builders +
///         UniswapX V2 order signing, so the hermetic and fork suites differ only in how they obtain the
///         reactor/Permit2 (locally deployed vs. real mainnet addresses).
/// @dev `_deploy()` wires the maker side and the filler; each concrete suite sets `reactor`/`permit2` first.
abstract contract UniswapXAquaFillerHarness is AquaStrategyBuilders, PermitSignature, DeployPermit2 {
    using OrderInfoBuilder for OrderInfo;
    using ProgramBuilder for Program;
    using V2DutchOrderLib for V2DutchOrder;

    AquaSwapVMRouter internal swapVm;
    IPermit2 internal permit2;
    V2DutchOrderReactor internal reactor;
    UniswapXAquaFiller internal filler;

    uint256 internal constant SWAPPER_PK = 0x1010;
    uint256 internal constant COSIGNER_PK = 0x2020;
    uint256 internal constant POLICY_SIGNER_PK = 0x3030;
    address internal swapper;

    // tokenA = input the swapper pays (USDC-like); tokenB = output the maker sources (WETH-like).
    uint256 internal nonce;
    uint256 internal policyNonce;

    event Swept(address indexed token, address indexed to, uint256 amount);

    constructor() AquaStrategyBuilders(address(aqua)) { }

    /// @dev Deploy the maker side (Aqua router, tokens, maker) and the filler. `reactor`/`permit2` must
    ///      already be set by the caller.
    function _deploy() internal {
        super.setUp(); // sets maker, tokenA, tokenB
        swapVm = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        filler = new UniswapXAquaFiller(
            address(this), ISwapVM(address(swapVm)), IReactor(address(reactor)), vm.addr(POLICY_SIGNER_PK)
        );
        filler.setTokenAllowed(address(tokenA), true);
        filler.setTokenAllowed(address(tokenB), true);

        swapper = vm.addr(SWAPPER_PK);
        vm.prank(swapper);
        tokenA.approve(address(permit2), type(uint256).max);
    }

    /// @dev Ship an XYC Aqua strategy for `mkr` with the given virtual reserves, and mint it `realOut` of
    ///      the output token so Aqua's `pull` can settle.
    function _shipXycMaker(
        address mkr,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 reserveIn,
        uint256 reserveOut,
        uint256 realOut
    )
        internal
        returns (ISwapVM.Order memory order, bytes32 strategyHash)
    {
        order = _protectedXycOrder(mkr);
        strategyHash = shipStrategy(swapVm, order, tokenIn, tokenOut, reserveIn, reserveOut);
        tokenOut.mint(mkr, realOut);
        filler.setTokenAllowed(address(tokenIn), true);
        filler.setTokenAllowed(address(tokenOut), true);
    }

    function _protectedXycOrder(address mkr) internal returns (ISwapVM.Order memory order) {
        maker = mkr; // createStrategy reads the inherited maker
        MakerSetup memory setup = MakerSetup({
            balanceA: 0,
            balanceB: 0,
            priceMin: 0,
            priceMax: 0,
            protocolFeeBps: 0,
            feeInBps: 0,
            protocolFeeRecipient: address(0),
            swapType: SwapType.XYC
        });
        Program memory program = ProgramBuilder.init(_opcodes());
        order = createStrategy(
            bytes.concat(
                program.build(
                    Controls._onlyTakerTokenBalanceNonZero,
                    ControlsArgsBuilder.buildTakerTokenBalanceNonZero(address(filler.TAKER_CREDENTIAL()))
                ),
                buildProgram(setup)
            )
        );
    }

    function _source(
        bytes32 contextHash,
        ISwapVM.Order memory order,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 amountOut,
        uint256 amountInMaximum
    )
        internal
        returns (UniswapXAquaFiller.SourceSwap memory source)
    {
        UniswapXAquaFiller.Authorization memory authorization = UniswapXAquaFiller.Authorization({
            kind: UniswapXAquaFiller.ExecutionKind.UserFill,
            nonce: policyNonce++,
            contextHash: contextHash,
            strategyHash: swapVm.hash(order),
            maker: order.maker,
            tokenIn: address(tokenIn),
            tokenOut: address(tokenOut),
            amountOut: amountOut,
            amountInLimit: amountInMaximum,
            rebateAmount: 0,
            deadlineBlock: uint64(block.number + 100)
        });
        source = UniswapXAquaFiller.SourceSwap({
            order: order, authorization: authorization, policySignature: _signAuthorization(authorization)
        });
    }

    function _rebateAuthorization(
        bytes32 contextHash,
        ISwapVM.Order memory order,
        TokenMock tokenIn,
        TokenMock tokenOut,
        uint256 amountOut,
        uint256 amountIn,
        uint256 rebateAmount
    )
        internal
        returns (UniswapXAquaFiller.Authorization memory authorization, bytes memory signature)
    {
        authorization = UniswapXAquaFiller.Authorization({
            kind: UniswapXAquaFiller.ExecutionKind.Rebate,
            nonce: policyNonce++,
            contextHash: contextHash,
            strategyHash: swapVm.hash(order),
            maker: order.maker,
            tokenIn: address(tokenIn),
            tokenOut: address(tokenOut),
            amountOut: amountOut,
            amountInLimit: amountIn,
            rebateAmount: rebateAmount,
            deadlineBlock: uint64(block.number + 100)
        });
        signature = _signAuthorization(authorization);
    }

    function _signAuthorization(UniswapXAquaFiller.Authorization memory authorization)
        internal
        view
        returns (bytes memory)
    {
        bytes32 digest = filler.hashAuthorization(authorization);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(POLICY_SIGNER_PK, digest);
        return bytes.concat(r, s, bytes1(v));
    }

    function _userFillContext(SignedOrder memory order) internal pure returns (bytes32) {
        return keccak256(abi.encode(order));
    }

    function _userFillBatchContext(SignedOrder[] memory orders) internal pure returns (bytes32) {
        return keccak256(abi.encode(orders));
    }

    function _outputs(
        TokenMock token,
        uint256 amount,
        address recipient
    )
        internal
        pure
        returns (DutchOutput[] memory outputs)
    {
        outputs = new DutchOutput[](1);
        outputs[0] =
            DutchOutput({ token: address(token), startAmount: amount, endAmount: amount, recipient: recipient });
    }

    function _signOrder(uint256 inputAmount, DutchOutput[] memory outputs)
        internal
        returns (SignedOrder memory signed)
    {
        OrderInfo memory info = OrderInfoBuilder.init(address(reactor)).withSwapper(swapper)
            .withDeadline(block.timestamp + 1000).withNonce(nonce++);

        CosignerData memory cosignerData = CosignerData({
            decayStartTime: block.timestamp,
            decayEndTime: info.deadline,
            exclusiveFiller: address(0),
            exclusivityOverrideBps: 0,
            inputAmount: 0,
            outputAmounts: new uint256[](outputs.length)
        });

        V2DutchOrder memory order = V2DutchOrder({
            info: info,
            cosigner: vm.addr(COSIGNER_PK),
            baseInput: DutchInput({
                token: SolERC20(address(tokenA)), startAmount: inputAmount, endAmount: inputAmount
            }),
            baseOutputs: outputs,
            cosignerData: cosignerData,
            cosignature: ""
        });

        bytes32 orderHash = order.hash();
        order.cosignature = _cosign(orderHash, cosignerData);
        signed = SignedOrder({ order: abi.encode(order), sig: signOrder(SWAPPER_PK, address(permit2), order) });
    }

    function _cosign(bytes32 orderHash, CosignerData memory cosignerData) internal pure returns (bytes memory) {
        bytes32 msgHash = keccak256(abi.encodePacked(orderHash, abi.encode(cosignerData)));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(COSIGNER_PK, msgHash);
        return bytes.concat(r, s, bytes1(v));
    }

    function _mintSwapper(uint256 inputAmount) internal {
        tokenA.mint(swapper, inputAmount);
    }

    /// @dev Exact input the SwapVM would charge for an exact-out swap — 22 zero bytes = exact-out taker
    ///      traits with no threshold/hooks. Uses the Simulator's `asView()` staticcall wrapper.
    function _quoteAmountIn(
        ISwapVM.Order memory order,
        address tokenIn,
        address tokenOut,
        uint256 amountOut
    )
        internal
        returns (uint256 amountIn)
    {
        bytes memory takerData = new bytes(22);
        vm.prank(address(filler));
        (amountIn,,) = swapVm.quote(order, tokenIn, tokenOut, amountOut, takerData);
    }
}
