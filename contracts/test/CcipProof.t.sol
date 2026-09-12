// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";

import { IAny2EVMMessageReceiver } from "@chainlink/contracts-ccip/contracts/interfaces/IAny2EVMMessageReceiver.sol";
import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";

import { RouteKind } from "../src/crosschain/CrossChainTypes.sol";
import { ProofHashLib, ProofKind } from "../src/crosschain/ProofTypes.sol";
import { IFillProofVerifier } from "../src/interfaces/IFillProofVerifier.sol";
import { IRepaymentProofVerifier } from "../src/interfaces/IRepaymentProofVerifier.sol";
import { CcipProofInbox } from "../src/proof/CcipProofInbox.sol";
import { CcipProofOutbox } from "../src/proof/CcipProofOutbox.sol";

contract MockCcipRouter is IRouterClient {
    uint256 public constant FEE = 0.01 ether;
    uint256 private _nonce;
    mapping(bytes32 messageId => bytes) public receivers;
    mapping(bytes32 messageId => bytes) public payloads;

    function isChainSupported(uint64) external pure returns (bool) {
        return true;
    }

    function getFee(uint64, Client.EVM2AnyMessage memory) external pure returns (uint256) {
        return FEE;
    }

    function ccipSend(uint64 selector, Client.EVM2AnyMessage calldata message) external payable returns (bytes32 id) {
        require(msg.value == FEE);
        id = keccak256(abi.encode(msg.sender, selector, ++_nonce, message.data));
        receivers[id] = message.receiver;
        payloads[id] = message.data;
    }

    function deliver(bytes32 messageId, uint64 sourceSelector, address sourceOutbox) external {
        _deliver(messageId, sourceSelector, sourceOutbox, receivers[messageId], payloads[messageId]);
    }

    function deliverPayload(
        bytes32 messageId,
        uint64 sourceSelector,
        address sourceOutbox,
        address receiver,
        bytes calldata payload
    )
        external
    {
        _deliver(messageId, sourceSelector, sourceOutbox, abi.encode(receiver), payload);
    }

    function _deliver(
        bytes32 messageId,
        uint64 sourceSelector,
        address sourceOutbox,
        bytes memory receiver,
        bytes memory payload
    )
        private
    {
        Client.EVMTokenAmount[] memory noTokens = new Client.EVMTokenAmount[](0);
        Client.Any2EVMMessage memory message = Client.Any2EVMMessage({
            messageId: messageId,
            sourceChainSelector: sourceSelector,
            sender: abi.encode(sourceOutbox),
            data: payload,
            destTokenAmounts: noTokens
        });
        IAny2EVMMessageReceiver(abi.decode(receiver, (address))).ccipReceive(message);
    }
}

contract CcipProofTest is Test {
    uint64 internal constant SOURCE_SELECTOR = 11;
    uint64 internal constant DESTINATION_SELECTOR = 22;

    MockCcipRouter internal router;
    CcipProofOutbox internal outbox;
    CcipProofInbox internal inbox;

    function setUp() public {
        router = new MockCcipRouter();
        address predictedOutbox = vm.computeCreateAddress(address(this), vm.getNonce(address(this)) + 1);
        inbox = new CcipProofInbox(address(router), SOURCE_SELECTOR, predictedOutbox);
        outbox = new CcipProofOutbox(router, address(this), DESTINATION_SELECTOR, address(inbox), 500_000);
        assertEq(address(outbox), predictedOutbox);
        vm.deal(address(this), 1 ether);
    }

    function test_fillProofDispatchesAndVerifiesByStoredId() public {
        IFillProofVerifier.VerifiedFill memory fill = _fill(keccak256("order"), keccak256("fill"));
        bytes memory payload = abi.encode(fill);
        bytes memory envelope = ProofHashLib.encode(ProofKind.Fill, payload);

        bytes32 payloadHash = outbox.record(fill.orderId, ProofKind.Fill, payload);
        assertEq(payloadHash, keccak256(envelope));

        bytes32 messageId = outbox.dispatch{ value: router.FEE() }(fill.orderId, envelope);
        router.deliver(messageId, SOURCE_SELECTOR, address(outbox));

        IFillProofVerifier.VerifiedFill memory verified = inbox.verifyFill(abi.encode(fill.fillId));
        assertEq(verified.orderId, fill.orderId);
        assertEq(verified.fillId, fill.fillId);
        assertEq(verified.destinationMaker, fill.destinationMaker);
    }

    function test_repaymentProofDispatchesAndVerifiesByStoredId() public {
        IRepaymentProofVerifier.VerifiedRepayment memory repayment = IRepaymentProofVerifier.VerifiedRepayment({
            orderId: keccak256("order"),
            originChainId: 42_161,
            originSettler: address(0x1234),
            maker: address(0x5678),
            repaymentToken: address(0x9abc),
            repaymentAmount: 10 ether,
            repaymentId: keccak256("repayment")
        });
        bytes memory payload = abi.encode(repayment);
        bytes memory envelope = ProofHashLib.encode(ProofKind.DirectRepayment, payload);

        outbox.record(repayment.orderId, ProofKind.DirectRepayment, payload);
        bytes32 messageId = outbox.dispatch{ value: router.FEE() }(repayment.orderId, envelope);
        router.deliver(messageId, SOURCE_SELECTOR, address(outbox));

        IRepaymentProofVerifier.VerifiedRepayment memory verified =
            inbox.verifyRepayment(abi.encode(repayment.repaymentId));
        assertEq(verified.orderId, repayment.orderId);
        assertEq(verified.repaymentId, repayment.repaymentId);
        assertEq(verified.repaymentAmount, repayment.repaymentAmount);
    }

    function test_rejectsWrongRecorderAndConflictingPayload() public {
        IFillProofVerifier.VerifiedFill memory fill = _fill(keccak256("order"), keccak256("fill"));
        vm.prank(address(0xbad));
        vm.expectRevert(abi.encodeWithSelector(CcipProofOutbox.NotRecorder.selector, address(0xbad)));
        outbox.record(fill.orderId, ProofKind.Fill, abi.encode(fill));

        outbox.record(fill.orderId, ProofKind.Fill, abi.encode(fill));
        fill.outputAmount++;
        bytes32 supplied = keccak256(ProofHashLib.encode(ProofKind.Fill, abi.encode(fill)));
        vm.expectRevert(
            abi.encodeWithSelector(
                CcipProofOutbox.PayloadConflict.selector, fill.orderId, outbox.recordedPayloads(fill.orderId), supplied
            )
        );
        outbox.record(fill.orderId, ProofKind.Fill, abi.encode(fill));
    }

    function test_recorderCanBeInitializedExactlyOnce() public {
        CcipProofOutbox initializing =
            new CcipProofOutbox(router, address(0), DESTINATION_SELECTOR, address(inbox), 500_000);
        initializing.initializeRecorder(address(this));
        assertEq(initializing.recorder(), address(this));

        vm.expectRevert(CcipProofOutbox.RecorderAlreadyInitialized.selector);
        initializing.initializeRecorder(address(this));
    }

    function test_sourceOutboxCanBeInitializedExactlyOnce() public {
        CcipProofInbox initializing = new CcipProofInbox(address(router), SOURCE_SELECTOR, address(0));
        initializing.initializeSourceOutbox(address(outbox));
        assertEq(initializing.sourceOutbox(), address(outbox));

        vm.expectRevert(CcipProofInbox.SourceOutboxAlreadyInitialized.selector);
        initializing.initializeSourceOutbox(address(outbox));
    }

    function test_destinationInboxCanBeInitializedExactlyOnce() public {
        CcipProofOutbox initializing =
            new CcipProofOutbox(router, address(this), DESTINATION_SELECTOR, address(0), 500_000);
        initializing.initializeDestinationInbox(address(inbox));
        assertEq(initializing.destinationInbox(), address(inbox));

        vm.expectRevert(CcipProofOutbox.DestinationInboxAlreadyInitialized.selector);
        initializing.initializeDestinationInbox(address(inbox));
    }

    function test_rejectsWrongFeeEnvelopeRemoteAndReplay() public {
        IFillProofVerifier.VerifiedFill memory fill = _fill(keccak256("order"), keccak256("fill"));
        bytes memory envelope = ProofHashLib.encode(ProofKind.Fill, abi.encode(fill));
        uint256 ccipFee = router.FEE();
        outbox.record(fill.orderId, ProofKind.Fill, abi.encode(fill));

        vm.expectRevert(abi.encodeWithSelector(CcipProofOutbox.IncorrectFee.selector, ccipFee, 0));
        outbox.dispatch(fill.orderId, envelope);

        bytes memory wrongEnvelope =
            ProofHashLib.encode(ProofKind.Fill, abi.encode(_fill(fill.orderId, keccak256("x"))));
        vm.expectRevert(
            abi.encodeWithSelector(
                CcipProofOutbox.PayloadConflict.selector,
                fill.orderId,
                outbox.recordedPayloads(fill.orderId),
                keccak256(wrongEnvelope)
            )
        );
        outbox.dispatch{ value: ccipFee }(fill.orderId, wrongEnvelope);

        bytes32 messageId = outbox.dispatch{ value: ccipFee }(fill.orderId, envelope);
        vm.expectRevert(abi.encodeWithSelector(CcipProofInbox.InvalidRemote.selector, SOURCE_SELECTOR + 1, address(0)));
        router.deliver(messageId, SOURCE_SELECTOR + 1, address(outbox));
        vm.expectRevert(abi.encodeWithSelector(CcipProofInbox.InvalidRemote.selector, SOURCE_SELECTOR, address(0xbad)));
        router.deliver(messageId, SOURCE_SELECTOR, address(0xbad));

        router.deliver(messageId, SOURCE_SELECTOR, address(outbox));
        vm.expectRevert(abi.encodeWithSelector(CcipProofInbox.DuplicateMessage.selector, messageId));
        router.deliver(messageId, SOURCE_SELECTOR, address(outbox));
    }

    function test_rejectsSecondProofForOneOrder() public {
        IFillProofVerifier.VerifiedFill memory first = _fill(keccak256("order"), keccak256("fill-1"));
        bytes memory firstEnvelope = ProofHashLib.encode(ProofKind.Fill, abi.encode(first));
        outbox.record(first.orderId, ProofKind.Fill, abi.encode(first));
        bytes32 firstMessage = outbox.dispatch{ value: router.FEE() }(first.orderId, firstEnvelope);
        router.deliver(firstMessage, SOURCE_SELECTOR, address(outbox));

        IFillProofVerifier.VerifiedFill memory second = _fill(first.orderId, keccak256("fill-2"));
        bytes memory secondEnvelope = ProofHashLib.encode(ProofKind.Fill, abi.encode(second));
        bytes32 secondMessage = keccak256("second-message");
        vm.expectRevert(abi.encodeWithSelector(CcipProofInbox.ProofConflict.selector, first.orderId));
        router.deliverPayload(secondMessage, SOURCE_SELECTOR, address(outbox), address(inbox), secondEnvelope);
    }

    function _fill(bytes32 orderId, bytes32 fillId) private pure returns (IFillProofVerifier.VerifiedFill memory) {
        return IFillProofVerifier.VerifiedFill({
            orderId: orderId,
            routeKind: RouteKind.DirectMaker,
            destinationChainId: 8453,
            destinationApp: address(0x1111),
            recipient: address(0x2222),
            outputToken: address(0x3333),
            outputAmount: 1 ether,
            destinationMaker: address(0x4444),
            destinationStrategyHash: keccak256("destination-strategy"),
            originStrategyHash: keccak256("origin-strategy"),
            repaymentToken: address(0x5555),
            repaymentAmount: 10 ether,
            maxCctpFee: 0,
            makerQuoteHash: keccak256("maker-quote"),
            fillId: fillId
        });
    }
}
