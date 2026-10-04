// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IERC8004Identity, IERC8004Validation} from "./interfaces/IERC8004.sol";
import {KineticRegistry} from "./KineticRegistry.sol";

/// @notice ERC-8004 validator for a device action.
/// The device signs an `ActionProof`. This contract checks that signature
/// against the key in `KineticRegistry`, then writes the validation.
/// The owner must approve this contract as operator on the agent NFT.
/// This contract never approves the device key, and it never calls `giveFeedback`.
contract KineticAttestor is EIP712 {
    bytes32 private constant ACTION_TYPEHASH =
        keccak256("ActionProof(uint256 agentId,bytes32 action,bytes32 paramsHash,uint256 timestamp,uint256 nonce)");

    uint256 public constant MAX_PROOF_AGE = 10 minutes;
    uint256 public constant MAX_FUTURE_SKEW = 2 minutes;
    uint8 public constant VERIFIED_RESPONSE = 100;
    string public constant ACTION_TAG = "kinetic-action";

    struct ActionProof {
        uint256 agentId;
        bytes32 action;
        bytes32 paramsHash;
        uint256 timestamp;
        uint256 nonce;
    }

    error ZeroAddress();
    error NotBound();
    error BadSignature();
    error StaleProof();
    error FutureProof();
    error NonceUsed();
    error NotOperator();

    event Attested(uint256 indexed agentId, address indexed device, bytes32 indexed requestHash, bytes32 action);

    KineticRegistry public immutable registry;
    IERC8004Identity public immutable identity;
    IERC8004Validation public immutable validation;

    mapping(uint256 agentId => mapping(uint256 nonce => bool used)) public nonceUsed;

    constructor(address registry_, address validationRegistry) EIP712("KineticAttestor", "1") {
        if (registry_ == address(0) || validationRegistry == address(0)) revert ZeroAddress();
        registry = KineticRegistry(registry_);
        identity = KineticRegistry(registry_).identity();
        validation = IERC8004Validation(validationRegistry);
    }

    /// @notice Submit a device-signed action. Anyone can relay the signature.
    function attest(ActionProof calldata proof, string calldata requestURI, bytes calldata signature) external {
        address device = registry.agentDevice(proof.agentId);
        if (device == address(0)) revert NotBound();
        if (nonceUsed[proof.agentId][proof.nonce]) revert NonceUsed();
        if (proof.timestamp + MAX_PROOF_AGE < block.timestamp) revert StaleProof();
        if (proof.timestamp > block.timestamp + MAX_FUTURE_SKEW) revert FutureProof();

        bytes32 digest = _digest(proof);
        (address signer, ECDSA.RecoverError err,) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || signer != device) revert BadSignature();

        address owner = identity.ownerOf(proof.agentId);
        if (!identity.isApprovedForAll(owner, address(this)) && identity.getApproved(proof.agentId) != address(this)) {
            revert NotOperator();
        }

        nonceUsed[proof.agentId][proof.nonce] = true;
        bytes32 requestHash =
            keccak256(abi.encode(proof.agentId, proof.action, proof.paramsHash, proof.timestamp, proof.nonce));

        validation.validationRequest(address(this), proof.agentId, requestURI, requestHash);
        validation.validationResponse(requestHash, VERIFIED_RESPONSE, "", proof.paramsHash, ACTION_TAG);
        emit Attested(proof.agentId, device, requestHash, proof.action);
    }

    function hashActionProof(ActionProof calldata proof) external view returns (bytes32) {
        return _digest(proof);
    }

    function _digest(ActionProof calldata proof) private view returns (bytes32) {
        return _hashTypedDataV4(
            keccak256(
                abi.encode(ACTION_TYPEHASH, proof.agentId, proof.action, proof.paramsHash, proof.timestamp, proof.nonce)
            )
        );
    }
}
