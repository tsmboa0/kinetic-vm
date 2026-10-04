// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

interface IDeviceVault {
    function initialize(uint256 agentId, uint256 perTxCap, uint256 dailyCap) external;

    function pauseFromRegistry(uint256 agentId) external;
}
