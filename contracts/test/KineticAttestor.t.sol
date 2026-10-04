// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {KineticTestBase} from "./KineticTestBase.sol";
import {KineticAttestor} from "../src/KineticAttestor.sol";

contract KineticAttestorTest is KineticTestBase {
    uint256 internal agentId;

    function setUp() public override {
        super.setUp();
        agentId = _claim();
    }

    function test_attest_requires_the_owner_to_approve_the_attestor_not_the_device() public {
        KineticAttestor.ActionProof memory proof = _proof(1);
        bytes memory sig = _sign(devicePk, attestor.hashActionProof(proof));

        vm.expectRevert(KineticAttestor.NotOperator.selector);
        attestor.attest(proof, "ipfs://action", sig);

        vm.prank(owner);
        identity.setApprovalForAll(address(attestor), true);
        attestor.attest(proof, "ipfs://action", sig);

        bytes32 requestHash =
            keccak256(abi.encode(proof.agentId, proof.action, proof.paramsHash, proof.timestamp, proof.nonce));
        assertEq(validation.validatorOf(requestHash), address(attestor));
        assertEq(validation.responseOf(requestHash), attestor.VERIFIED_RESPONSE());
        assertEq(validation.responseHashOf(requestHash), proof.paramsHash);
        assertEq(validation.tagOf(requestHash), attestor.ACTION_TAG());
        assertTrue(validation.responded(requestHash));
        assertFalse(identity.isApprovedForAll(owner, device));
        assertTrue(identity.isApprovedForAll(owner, address(attestor)));
    }

    function test_attest_rejects_replay_a_stale_proof_and_the_wrong_signer() public {
        vm.prank(owner);
        identity.setApprovalForAll(address(attestor), true);

        KineticAttestor.ActionProof memory proof = _proof(7);
        bytes memory sig = _sign(devicePk, attestor.hashActionProof(proof));
        attestor.attest(proof, "", sig);

        vm.expectRevert(KineticAttestor.NonceUsed.selector);
        attestor.attest(proof, "", sig);

        KineticAttestor.ActionProof memory stale = _proof(8);
        stale.timestamp = block.timestamp - attestor.MAX_PROOF_AGE() - 1;
        bytes memory staleSig = _sign(devicePk, attestor.hashActionProof(stale));
        vm.expectRevert(KineticAttestor.StaleProof.selector);
        attestor.attest(stale, "", staleSig);

        KineticAttestor.ActionProof memory forged = _proof(9);
        bytes memory otherSig = _sign(otherPk, attestor.hashActionProof(forged));
        vm.expectRevert(KineticAttestor.BadSignature.selector);
        attestor.attest(forged, "", otherSig);
    }

    function test_revoked_device_cannot_attest() public {
        vm.prank(owner);
        identity.setApprovalForAll(address(attestor), true);
        vm.prank(owner);
        registry.revoke(agentId);

        KineticAttestor.ActionProof memory proof = _proof(1);
        bytes memory sig = _sign(devicePk, attestor.hashActionProof(proof));
        vm.expectRevert(KineticAttestor.NotBound.selector);
        attestor.attest(proof, "", sig);
    }

    function _proof(uint256 nonce) internal view returns (KineticAttestor.ActionProof memory) {
        return KineticAttestor.ActionProof({
            agentId: agentId,
            action: keccak256("vend"),
            paramsHash: keccak256("slot-1"),
            timestamp: block.timestamp,
            nonce: nonce
        });
    }
}
