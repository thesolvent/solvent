// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { AquaStrategyBuilders } from "@1inch/swap-vm/test/base/AquaStrategyBuilders.sol";
import { AquaSwapVMRouter } from "@1inch/swap-vm/src/routers/AquaSwapVMRouter.sol";
import { ISwapVM } from "@1inch/swap-vm/src/interfaces/ISwapVM.sol";
import { Controls, ControlsArgsBuilder } from "@1inch/swap-vm/src/instructions/Controls.sol";
import { Program, ProgramBuilder } from "@1inch/swap-vm/test/utils/ProgramBuilder.sol";

import { DeployPermit2 } from "uniswapx-test/util/DeployPermit2.sol";
import { ISignatureTransfer } from "permit2/src/interfaces/ISignatureTransfer.sol";

import { Erc7683AquaFiller } from "../src/Erc7683AquaFiller.sol";
import { SolventSameChainSettler } from "../src/SolventSameChainSettler.sol";
import { SolventTakerCredential } from "../src/SolventTakerCredential.sol";
import { Solvent7683Order } from "../src/interfaces/ISolvent7683.sol";

contract Erc7683CredentialPeer { }

contract Erc7683AquaFillerTest is AquaStrategyBuilders, DeployPermit2 {
    using ProgramBuilder for Program;

    bytes internal constant WITNESS_STUB =
        "PermitWitnessTransferFrom(TokenPermissions permitted,address spender,uint256 nonce,uint256 deadline,";
    bytes32 internal constant TOKEN_PERMISSIONS_TYPEHASH = keccak256("TokenPermissions(address token,uint256 amount)");

    uint256 internal constant USER_KEY = 0x1010;
    uint256 internal constant POLICY_SIGNER_KEY = 0x3030;
    uint256 internal constant OUTPUT_AMOUNT = 1 ether;
    uint256 internal constant EXECUTOR_FEE = 10 ether;

    AquaSwapVMRouter internal swapVm;
    ISignatureTransfer internal permit2;
    SolventSameChainSettler internal settler;
    SolventTakerCredential internal credential;
    Erc7683AquaFiller internal filler;
    address internal user;
    address internal executor = address(0xE0E0);
    uint256 internal policyNonce;

    constructor() AquaStrategyBuilders(address(aqua)) { }

    function setUp() public override {
        super.setUp();
        permit2 = ISignatureTransfer(deployPermit2());
        swapVm = new AquaSwapVMRouter(address(aqua), address(0), address(this), "SwapVM", "1.0.0");
        credential = new SolventTakerCredential(address(this));
        settler = new SolventSameChainSettler(permit2, credential);
        filler = new Erc7683AquaFiller(
            address(this), ISwapVM(address(swapVm)), settler, credential, vm.addr(POLICY_SIGNER_KEY)
        );
        credential.setTaker(address(filler), true);
        credential.setTaker(address(new Erc7683CredentialPeer()), true);
        credential.freeze();
        filler.setTokenAllowed(address(tokenA), true);
        filler.setTokenAllowed(address(tokenB), true);

        user = vm.addr(USER_KEY);
        vm.prank(user);
        tokenA.approve(address(permit2), type(uint256).max);
        vm.warp(1_000_000);
    }

    function test_fillProtectedStrategySettlesUserAndPaysExecutorWithoutInventory() public {
        ISwapVM.Order memory makerOrder = _shipXyc(true);
        uint256 amountIn = _quoteAmountIn(makerOrder, OUTPUT_AMOUNT);
        Solvent7683Order memory order = _order(amountIn + EXECUTOR_FEE, 0);
        bytes memory orderData = abi.encode(order);
        Erc7683AquaFiller.SourceSwap[] memory sources = _sources(order, makerOrder, amountIn, executor);
        tokenA.mint(user, order.inputAmount);

        uint256 earned = filler.fill(orderData, _signOrder(order), abi.encode(sources), executor);

        assertEq(earned, EXECUTOR_FEE);
        assertEq(tokenB.balanceOf(user), OUTPUT_AMOUNT);
        assertEq(tokenA.balanceOf(executor), EXECUTOR_FEE);
        assertEq(tokenA.balanceOf(address(filler)), 0);
        assertEq(tokenB.balanceOf(address(filler)), 0);
        assertEq(tokenA.balanceOf(address(settler)), 0);
        assertEq(tokenB.balanceOf(address(settler)), 0);
        assertTrue(filler.isNonceUsed(sources[0].authorization.nonce));
    }

    function test_fillReservesTheSignedExecutorFeeBeforeExecution() public {
        ISwapVM.Order memory makerOrder = _shipXyc(true);
        uint256 amountIn = _quoteAmountIn(makerOrder, OUTPUT_AMOUNT);
        Solvent7683Order memory order = _order(amountIn + EXECUTOR_FEE - 1, 1);
        Erc7683AquaFiller.SourceSwap[] memory sources = _sources(order, makerOrder, amountIn, executor);
        bytes memory signature = _signOrder(order);

        vm.expectRevert(abi.encodeWithSelector(Erc7683AquaFiller.InputLimitExceeded.selector, amountIn, amountIn - 1));
        filler.fill(abi.encode(order), signature, abi.encode(sources), executor);
    }

    function test_fillRejectsStrategyWithoutTheSharedCredentialOpcode() public {
        ISwapVM.Order memory makerOrder = _shipXyc(false);
        uint256 amountIn = _quoteAmountIn(makerOrder, OUTPUT_AMOUNT);
        Solvent7683Order memory order = _order(amountIn + EXECUTOR_FEE, 2);
        Erc7683AquaFiller.SourceSwap[] memory sources = _sources(order, makerOrder, amountIn, executor);
        bytes memory signature = _signOrder(order);

        vm.expectRevert(Erc7683AquaFiller.CredentialGateMissing.selector);
        filler.fill(abi.encode(order), signature, abi.encode(sources), executor);
    }

    function test_sourceAuthorizationBindsTheExecutorPaymentRecipient() public {
        ISwapVM.Order memory makerOrder = _shipXyc(true);
        uint256 amountIn = _quoteAmountIn(makerOrder, OUTPUT_AMOUNT);
        Solvent7683Order memory order = _order(amountIn + EXECUTOR_FEE, 3);
        Erc7683AquaFiller.SourceSwap[] memory sources = _sources(order, makerOrder, amountIn, executor);
        address changedRecipient = address(0xBEEF);
        bytes memory signature = _signOrder(order);
        bytes32 expectedContext = filler.userFillContext(settler.orderId(order), changedRecipient);

        vm.expectRevert(
            abi.encodeWithSelector(
                Erc7683AquaFiller.AuthorizationContextMismatch.selector,
                sources[0].authorization.contextHash,
                expectedContext
            )
        );
        filler.fill(abi.encode(order), signature, abi.encode(sources), changedRecipient);
    }

    function _shipXyc(bool protected) internal returns (ISwapVM.Order memory order) {
        bytes memory program = buildProgram(_xycSetup());
        if (protected) {
            Program memory builder = ProgramBuilder.init(_opcodes());
            program = bytes.concat(
                builder.build(
                    Controls._onlyTakerTokenBalanceNonZero,
                    ControlsArgsBuilder.buildTakerTokenBalanceNonZero(address(credential))
                ),
                program
            );
        }
        order = createStrategy(program);
        shipStrategy(swapVm, order, tokenA, tokenB, 3_000_000 ether, 1000 ether);
        tokenB.mint(maker, 10 ether);
    }

    function _xycSetup() internal pure returns (MakerSetup memory) {
        return MakerSetup({
            balanceA: 0,
            balanceB: 0,
            priceMin: 0,
            priceMax: 0,
            protocolFeeBps: 0,
            feeInBps: 0,
            protocolFeeRecipient: address(0),
            swapType: SwapType.XYC
        });
    }

    function _order(uint256 inputAmount, uint256 nonce) internal view returns (Solvent7683Order memory) {
        return Solvent7683Order({
            settler: address(settler),
            user: user,
            chainId: block.chainid,
            inputToken: address(tokenA),
            inputAmount: inputAmount,
            outputToken: address(tokenB),
            outputAmount: OUTPUT_AMOUNT,
            recipient: user,
            executorFee: EXECUTOR_FEE,
            nonce: nonce,
            deadline: block.timestamp + 1 hours
        });
    }

    function _sources(
        Solvent7683Order memory order,
        ISwapVM.Order memory makerOrder,
        uint256 amountIn,
        address paymentRecipient
    )
        internal
        returns (Erc7683AquaFiller.SourceSwap[] memory sources)
    {
        Erc7683AquaFiller.Authorization memory authorization = Erc7683AquaFiller.Authorization({
            kind: Erc7683AquaFiller.ExecutionKind.UserFill,
            nonce: policyNonce++,
            contextHash: filler.userFillContext(settler.orderId(order), paymentRecipient),
            strategyHash: swapVm.hash(makerOrder),
            maker: makerOrder.maker,
            tokenIn: address(tokenA),
            tokenOut: address(tokenB),
            amountOut: OUTPUT_AMOUNT,
            amountInLimit: amountIn,
            rebateAmount: 0,
            deadlineBlock: uint64(block.number + 100)
        });
        sources = new Erc7683AquaFiller.SourceSwap[](1);
        sources[0] = Erc7683AquaFiller.SourceSwap({
            order: makerOrder, authorization: authorization, policySignature: _signAuthorization(authorization)
        });
    }

    function _quoteAmountIn(ISwapVM.Order memory order, uint256 amountOut) internal returns (uint256 amountIn) {
        vm.prank(address(filler));
        (amountIn,,) = swapVm.quote(order, address(tokenA), address(tokenB), amountOut, new bytes(22));
    }

    function _signAuthorization(Erc7683AquaFiller.Authorization memory authorization)
        internal
        view
        returns (bytes memory)
    {
        bytes32 digest = filler.hashAuthorization(authorization);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(POLICY_SIGNER_KEY, digest);
        return bytes.concat(r, s, bytes1(v));
    }

    function _signOrder(Solvent7683Order memory order) internal view returns (bytes memory) {
        bytes32 tokenPermissions =
            keccak256(abi.encode(TOKEN_PERMISSIONS_TYPEHASH, order.inputToken, order.inputAmount));
        bytes32 witnessTypeHash = keccak256(abi.encodePacked(WITNESS_STUB, settler.WITNESS_TYPE_STRING()));
        bytes32 structHash = keccak256(
            abi.encode(
                witnessTypeHash, tokenPermissions, address(settler), order.nonce, order.deadline, settler.orderId(order)
            )
        );
        bytes32 digest = keccak256(abi.encodePacked("\x19\x01", permit2.DOMAIN_SEPARATOR(), structHash));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(USER_KEY, digest);
        return bytes.concat(r, s, bytes1(v));
    }
}
