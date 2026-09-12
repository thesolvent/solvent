// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Test } from "forge-std/Test.sol";
import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";

import { SolventTakerCredential } from "../src/SolventTakerCredential.sol";

contract TakerMock { }

contract SolventTakerCredentialTest is Test {
    SolventTakerCredential internal credential;
    TakerMock internal first;
    TakerMock internal second;

    function setUp() public {
        credential = new SolventTakerCredential(address(this));
        first = new TakerMock();
        second = new TakerMock();
    }

    function test_freezeActivatesExactlyTwoContractTakers() public {
        credential.setTaker(address(first), true);
        credential.setTaker(address(second), true);

        assertEq(credential.balanceOf(address(first)), 0, "mutable credentials stay inactive");
        credential.freeze();

        assertEq(credential.balanceOf(address(first)), 1);
        assertEq(credential.balanceOf(address(second)), 1);
        assertEq(credential.balanceOf(address(0xBEEF)), 0);
    }

    function test_freezeRejectsIncompleteConfiguration() public {
        credential.setTaker(address(first), true);
        vm.expectRevert(abi.encodeWithSelector(SolventTakerCredential.WrongTakerCount.selector, uint256(1), uint256(2)));
        credential.freeze();
    }

    function test_configurationRejectsEoaAndThirdTaker() public {
        vm.expectRevert(abi.encodeWithSelector(SolventTakerCredential.InvalidTaker.selector, address(0xBEEF)));
        credential.setTaker(address(0xBEEF), true);

        credential.setTaker(address(first), true);
        credential.setTaker(address(second), true);
        TakerMock third = new TakerMock();
        vm.expectRevert(SolventTakerCredential.TakerLimitReached.selector);
        credential.setTaker(address(third), true);
    }

    function test_frozenConfigurationCannotChange() public {
        credential.setTaker(address(first), true);
        credential.setTaker(address(second), true);
        credential.freeze();

        vm.expectRevert(SolventTakerCredential.AlreadyFrozen.selector);
        credential.setTaker(address(first), false);
    }

    function test_onlyOwnerCanConfigure() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(0xBEEF)));
        credential.setTaker(address(first), true);
    }
}
