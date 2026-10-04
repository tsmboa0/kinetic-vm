// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {KineticRegistry} from "../src/KineticRegistry.sol";
import {DeviceVault} from "../src/DeviceVault.sol";
import {KineticAttestor} from "../src/KineticAttestor.sol";
import {MockIdentityRegistry, MockValidationRegistry} from "./mocks/MockRegistries.sol";

contract KineticTestBase is Test {
    uint256 internal ownerPk = 0xA11CE;
    uint256 internal devicePk = 0xB0B;
    uint256 internal otherPk = 0xC0FFEE;

    address internal owner;
    address internal device;
    address internal other;

    MockIdentityRegistry internal identity;
    MockValidationRegistry internal validation;
    KineticRegistry internal registry;
    DeviceVault internal vault;
    KineticAttestor internal attestor;

    function setUp() public virtual {
        vm.warp(1_000_000);
        owner = vm.addr(ownerPk);
        device = vm.addr(devicePk);
        other = vm.addr(otherPk);
        identity = new MockIdentityRegistry();
        validation = new MockValidationRegistry(address(identity));
        registry = new KineticRegistry(address(identity));
        vault = new DeviceVault(address(registry));
        registry.setVault(address(vault));
        attestor = new KineticAttestor(address(registry), address(validation));
    }

    function _sign(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _deadline() internal view returns (uint256) {
        return block.timestamp + 1 hours;
    }

    function _claimSig(uint256 pk, address dev, address own) internal view returns (bytes memory) {
        return _sign(pk, registry.hashClaim(dev, own, registry.claimNonce(dev), _deadline()));
    }

    function _claim() internal returns (uint256 agentId) {
        bytes memory sig = _claimSig(devicePk, device, owner);
        vm.prank(owner);
        agentId = registry.claim(device, sig, _deadline(), "ipfs://agent", KineticRegistry.Limits(1 ether, 2 ether));
    }
}
