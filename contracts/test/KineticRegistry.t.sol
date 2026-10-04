// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {KineticTestBase} from "./KineticTestBase.sol";
import {KineticRegistry} from "../src/KineticRegistry.sol";

contract KineticRegistryTest is KineticTestBase {
    function test_claim_mints_to_the_owner_and_binds_the_device() public {
        uint256 agentId = _claim();

        assertEq(identity.ownerOf(agentId), owner);
        assertEq(registry.agentDevice(agentId), device);
        (uint256 found, bool bound) = registry.deviceToAgent(device);
        assertTrue(bound);
        assertEq(found, agentId);
        assertEq(registry.ownerOfAgent(agentId), owner);
        assertFalse(identity.isApprovedForAll(owner, device));
        assertEq(identity.getApproved(agentId), address(0));
        (uint256 perTx, uint256 daily, bool paused) = vault.limitsOf(agentId);
        assertEq(perTx, 1 ether);
        assertEq(daily, 2 ether);
        assertFalse(paused);
    }

    function test_claim_rejects_a_device_that_is_the_owner() public {
        bytes memory sig = _sign(ownerPk, registry.hashClaim(owner, owner, 0));
        vm.prank(owner);
        vm.expectRevert(KineticRegistry.DeviceIsOwner.selector);
        registry.claim(owner, sig, "ipfs://agent", KineticRegistry.Limits(1, 1));
    }

    function test_claim_rejects_a_ticket_signed_by_someone_else() public {
        bytes memory sig = _claimSig(otherPk, device, owner);
        vm.prank(owner);
        vm.expectRevert(KineticRegistry.BadSignature.selector);
        registry.claim(device, sig, "ipfs://agent", KineticRegistry.Limits(1, 1));
    }

    function test_claim_rejects_a_ticket_naming_a_different_owner() public {
        bytes memory sig = _claimSig(devicePk, device, other);
        vm.prank(owner);
        vm.expectRevert(KineticRegistry.BadSignature.selector);
        registry.claim(device, sig, "ipfs://agent", KineticRegistry.Limits(1, 1));
    }

    function test_claim_rejects_replay_of_a_spent_ticket() public {
        bytes memory spent = _claimSig(devicePk, device, owner);
        vm.prank(owner);
        uint256 agentId = registry.claim(device, spent, "ipfs://agent", KineticRegistry.Limits(1, 1));
        vm.prank(owner);
        registry.revoke(agentId);

        vm.prank(owner);
        vm.expectRevert(KineticRegistry.BadSignature.selector);
        registry.claim(device, spent, "ipfs://again", KineticRegistry.Limits(1, 1));
    }

    function test_link_binds_an_existing_agent_the_caller_owns() public {
        vm.prank(owner);
        uint256 agentId = identity.register("ipfs://existing");
        bytes memory sig = _claimSig(devicePk, device, owner);

        vm.prank(owner);
        registry.link(agentId, device, sig, KineticRegistry.Limits(5, 9));

        assertEq(registry.agentDevice(agentId), device);
        assertEq(identity.ownerOf(agentId), owner);
    }

    function test_link_rejects_a_non_owner() public {
        vm.prank(owner);
        uint256 agentId = identity.register("ipfs://existing");
        bytes memory sig = _claimSig(devicePk, device, other);

        vm.prank(other);
        vm.expectRevert(KineticRegistry.NotOwner.selector);
        registry.link(agentId, device, sig, KineticRegistry.Limits(1, 1));
    }

    function test_revoke_clears_the_device_and_pauses_spending() public {
        uint256 agentId = _claim();
        vm.prank(owner);
        registry.revoke(agentId);

        (uint256 found, bool bound) = registry.deviceToAgent(device);
        assertFalse(bound);
        assertEq(found, 0);
        assertEq(registry.agentDevice(agentId), address(0));
        (,, bool paused) = vault.limitsOf(agentId);
        assertTrue(paused);
        assertEq(identity.ownerOf(agentId), owner);
    }

    function test_rotate_replaces_the_device_key() public {
        uint256 agentId = _claim();
        address nextDevice = vm.addr(0xD1);
        bytes memory sig = _sign(0xD1, registry.hashClaim(nextDevice, owner, 0));

        vm.prank(owner);
        registry.rotate(agentId, nextDevice, sig);

        assertEq(registry.agentDevice(agentId), nextDevice);
        (, bool oldBound) = registry.deviceToAgent(device);
        assertFalse(oldBound);
        (, bool newBound) = registry.deviceToAgent(nextDevice);
        assertTrue(newBound);
    }

    function test_set_vault_is_once_and_deployer_only() public {
        vm.prank(owner);
        vm.expectRevert(KineticRegistry.NotDeployer.selector);
        registry.setVault(address(vault));

        vm.expectRevert(KineticRegistry.VaultAlreadySet.selector);
        registry.setVault(address(0x1234));
    }
}
