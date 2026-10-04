// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {KineticTestBase} from "./KineticTestBase.sol";
import {DeviceVault} from "../src/DeviceVault.sol";

contract DeviceVaultTest is KineticTestBase {
    uint256 internal agentId;
    address internal recipient = address(0xBEEF);

    function setUp() public override {
        super.setUp();
        agentId = _claim();
        vm.deal(owner, 10 ether);
        vm.prank(owner);
        vault.deposit{value: 4 ether}(agentId);
    }

    function test_pay_requires_the_device_the_allowlist_and_the_caps() public {
        vm.prank(device);
        vm.expectRevert(DeviceVault.NotAllowlisted.selector);
        vault.pay(agentId, recipient, 0.5 ether);

        vm.prank(owner);
        vault.allowRecipient(agentId, recipient, 0, "");

        vm.prank(owner);
        vm.expectRevert(DeviceVault.NotDevice.selector);
        vault.pay(agentId, recipient, 0.5 ether);

        vm.prank(device);
        vault.pay(agentId, recipient, 0.5 ether);
        assertEq(recipient.balance, 0.5 ether);
        assertEq(vault.balanceOf(agentId), 3.5 ether);
        assertEq(vault.spentToday(agentId), 0.5 ether);

        vm.prank(device);
        vm.expectRevert(DeviceVault.PerTxCap.selector);
        vault.pay(agentId, recipient, 1 ether + 1);
    }

    function test_daily_cap_counts_pay_and_gas_top_up() public {
        vm.prank(owner);
        vault.allowRecipient(agentId, recipient, 0, "");

        vm.prank(device);
        vault.pay(agentId, recipient, 1 ether);
        vm.prank(device);
        vault.topUpGas(agentId, 1 ether);
        assertEq(device.balance, 1 ether);

        vm.prank(device);
        vm.expectRevert(DeviceVault.DailyCap.selector);
        vault.pay(agentId, recipient, 1);

        vm.prank(device);
        vm.expectRevert(DeviceVault.NotAllowlisted.selector);
        vault.pay(agentId, other, 1);
    }

    function test_pause_stops_the_device_but_not_owner_withdraw() public {
        vm.prank(owner);
        vault.pause(agentId);

        vm.prank(device);
        vm.expectRevert(DeviceVault.Paused.selector);
        vault.topUpGas(agentId, 1);

        uint256 before = owner.balance;
        vm.prank(owner);
        vault.withdraw(agentId, 4 ether);
        assertEq(owner.balance, before + 4 ether);
        assertEq(vault.balanceOf(agentId), 0);
    }

    function test_device_cannot_withdraw_or_tighten_or_loosen() public {
        vm.prank(device);
        vm.expectRevert(DeviceVault.NotOwner.selector);
        vault.withdraw(agentId, 1);

        vm.prank(device);
        vm.expectRevert(DeviceVault.NotOwner.selector);
        vault.tightenCaps(agentId, 0.5 ether, 1 ether);

        vm.prank(device);
        vm.expectRevert(DeviceVault.BadSignature.selector);
        vault.loosenCaps(agentId, 2 ether, 3 ether, block.timestamp + 1 hours, "");
    }

    function test_owner_tightens_directly_and_loosens_with_a_signature() public {
        vm.prank(owner);
        vault.tightenCaps(agentId, 0.4 ether, 1 ether);
        (uint256 perTx, uint256 daily,) = vault.limitsOf(agentId);
        assertEq(perTx, 0.4 ether);
        assertEq(daily, 1 ether);

        vm.prank(owner);
        vm.expectRevert(DeviceVault.NotATighten.selector);
        vault.tightenCaps(agentId, 0.5 ether, 1 ether);

        uint256 nonce = vault.loosenNonce(agentId);
        uint256 deadline = block.timestamp + 1 hours;
        bytes32 digest = vault.hashLoosenCaps(agentId, 0.8 ether, 1.5 ether, nonce, deadline);
        bytes memory sig = _sign(ownerPk, digest);

        vm.prank(device);
        vault.loosenCaps(agentId, 0.8 ether, 1.5 ether, deadline, sig);
        (perTx, daily,) = vault.limitsOf(agentId);
        assertEq(perTx, 0.8 ether);
        assertEq(daily, 1.5 ether);

        vm.prank(device);
        vm.expectRevert(DeviceVault.BadSignature.selector);
        vault.loosenCaps(agentId, 0.9 ether, 1.5 ether, deadline, sig);
    }

    function test_owner_can_loosen_by_sending_the_transaction() public {
        vm.prank(owner);
        vault.loosenCaps(agentId, 2 ether, 4 ether, 0, "");
        (uint256 perTx, uint256 daily,) = vault.limitsOf(agentId);
        assertEq(perTx, 2 ether);
        assertEq(daily, 4 ether);
    }

    function test_revoke_blocks_a_later_payment() public {
        vm.prank(owner);
        vault.allowRecipient(agentId, recipient, 0, "");
        vm.prank(owner);
        registry.revoke(agentId);

        vm.prank(device);
        vm.expectRevert(DeviceVault.NotDevice.selector);
        vault.pay(agentId, recipient, 0.1 ether);
    }

    function test_raw_ether_is_rejected() public {
        vm.deal(owner, 1 ether);
        vm.prank(owner);
        (bool ok,) = address(vault).call{value: 1 ether}("");
        assertFalse(ok);
    }
}
