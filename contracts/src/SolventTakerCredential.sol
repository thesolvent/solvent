// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import { Ownable } from "@openzeppelin/contracts/access/Ownable.sol";
import { Ownable2Step } from "@openzeppelin/contracts/access/Ownable2Step.sol";

/// @title SolventTakerCredential
/// @notice A permanent, nontransferable credential shared by Solvent's protocol fillers.
/// @dev Protected strategies observe no holder until the complete taker set is frozen.
contract SolventTakerCredential is Ownable2Step {
    uint256 public constant REQUIRED_TAKERS = 2;

    mapping(address taker => bool authorized) public isTaker;
    uint256 public takerCount;
    bool public frozen;

    event TakerPermissionUpdated(address indexed taker, bool authorized);
    event Frozen();

    error AlreadyFrozen();
    error InvalidTaker(address taker);
    error TakerLimitReached();
    error TakerPermissionUnchanged(address taker, bool authorized);
    error OwnershipRenunciationDisabled();
    error WrongTakerCount(uint256 given, uint256 required);

    constructor(address owner_) Ownable(owner_) { }

    /// @notice Adds or removes a deployed filler before the credential is frozen.
    function setTaker(address taker, bool authorized) external onlyOwner {
        if (frozen) {
            revert AlreadyFrozen();
        }
        if (taker == address(0) || taker.code.length == 0) {
            revert InvalidTaker(taker);
        }
        if (isTaker[taker] == authorized) {
            revert TakerPermissionUnchanged(taker, authorized);
        }

        if (authorized) {
            if (takerCount == REQUIRED_TAKERS) {
                revert TakerLimitReached();
            }
            unchecked {
                ++takerCount;
            }
        } else {
            unchecked {
                --takerCount;
            }
        }
        isTaker[taker] = authorized;
        emit TakerPermissionUpdated(taker, authorized);
    }

    /// @notice Permanently activates the configured filler set.
    function freeze() external onlyOwner {
        if (frozen) {
            revert AlreadyFrozen();
        }
        if (takerCount != REQUIRED_TAKERS) {
            revert WrongTakerCount(takerCount, REQUIRED_TAKERS);
        }
        frozen = true;
        emit Frozen();
    }

    /// @notice Prevents configuration from becoming unreachable before permanent activation.
    function renounceOwnership() public pure override {
        revert OwnershipRenunciationDisabled();
    }

    /// @notice Reports the credential only for an authorized filler after permanent activation.
    function balanceOf(address account) external view returns (uint256) {
        return frozen && isTaker[account] ? 1 : 0;
    }
}
