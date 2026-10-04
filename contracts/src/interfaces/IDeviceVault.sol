// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

interface IDeviceVault {
    /// @notice First bind opens the vault. A later bind, after revoke or release, replaces the caps and stays paused.
    function bindLimits(uint256 agentId, uint256 perTxCap, uint256 dailyCap) external;

    function pauseFromRegistry(uint256 agentId) external;
}
