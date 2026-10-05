// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {IERC721} from "@openzeppelin/contracts/token/ERC721/IERC721.sol";
import {KineticRegistry} from "../src/KineticRegistry.sol";
import {DeviceVault} from "../src/DeviceVault.sol";
import {KineticAttestor} from "../src/KineticAttestor.sol";
import {Monad8004} from "../src/Monad8004.sol";

/// @notice One live pass on Monad testnet: claim, fund, pay, attest.
/// Keys stay in the environment. Do not commit them.
contract Smoke is Script {
    function run() external {
        uint256 ownerKey = vm.envUint("MONAD_DEPLOYER_KEY");
        uint256 deviceKey = vm.envUint("DEVICE_KEY");
        address owner = vm.addr(ownerKey);
        address device = vm.addr(deviceKey);
        KineticRegistry registry = KineticRegistry(vm.envAddress("KINETIC_REGISTRY"));
        DeviceVault vault = DeviceVault(payable(vm.envAddress("DEVICE_VAULT")));
        KineticAttestor attestor = KineticAttestor(vm.envAddress("KINETIC_ATTESTOR"));

        uint256 agentId = _claimAndFund(ownerKey, deviceKey, device, owner, registry, vault, attestor);
        _pay(deviceKey, vault, agentId, owner);
        bytes32 requestHash = _attest(ownerKey, deviceKey, attestor, agentId);

        console.log("owner", owner);
        console.log("device", device);
        console.log("agentId", agentId);
        console.logBytes32(requestHash);
    }

    function _sign(uint256 key, bytes32 digest) private returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(key, digest);
        return abi.encodePacked(r, s, v);
    }

    function _claimAndFund(
        uint256 ownerKey,
        uint256 deviceKey,
        address device,
        address owner,
        KineticRegistry registry,
        DeviceVault vault,
        KineticAttestor attestor
    ) private returns (uint256 agentId) {
        uint256 deadline = block.timestamp + 1 hours;
        bytes memory claimSig =
            _sign(deviceKey, registry.hashClaim(device, owner, registry.claimNonce(device), deadline));
        vm.startBroadcast(ownerKey);
        agentId = registry.claim(
            device,
            claimSig,
            deadline,
            "kinetic-smoke",
            KineticRegistry.Limits({perTxCap: 0.05 ether, dailyCap: 0.2 ether})
        );
        vault.deposit{value: 0.1 ether}(agentId);
        vault.allowRecipient(agentId, owner, 0, "");
        vault.topUpGas(agentId, 0.05 ether);
        IERC721(Monad8004.TESTNET_IDENTITY).setApprovalForAll(address(attestor), true);
        vm.stopBroadcast();
    }

    function _pay(uint256 deviceKey, DeviceVault vault, uint256 agentId, address owner) private {
        vm.startBroadcast(deviceKey);
        vault.pay(agentId, owner, 0.02 ether);
        vm.stopBroadcast();
    }

    function _attest(uint256 ownerKey, uint256 deviceKey, KineticAttestor attestor, uint256 agentId)
        private
        returns (bytes32 requestHash)
    {
        KineticAttestor.ActionProof memory proof = KineticAttestor.ActionProof({
            agentId: agentId,
            action: keccak256("vend"),
            paramsHash: keccak256("slot-1"),
            requestURI: "ipfs://kinetic-smoke-action",
            timestamp: block.timestamp,
            nonce: 1
        });
        bytes memory actionSig = _sign(deviceKey, attestor.hashActionProof(proof));
        requestHash = keccak256(
            abi.encode(
                proof.agentId,
                proof.action,
                proof.paramsHash,
                keccak256(bytes(proof.requestURI)),
                proof.timestamp,
                proof.nonce
            )
        );
        vm.startBroadcast(ownerKey);
        attestor.attest(proof, actionSig);
        vm.stopBroadcast();
    }
}
