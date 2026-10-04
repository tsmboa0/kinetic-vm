// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {IERC721Receiver} from "@openzeppelin/contracts/token/ERC721/IERC721Receiver.sol";
import {IERC8004Identity} from "./interfaces/IERC8004.sol";
import {IDeviceVault} from "./interfaces/IDeviceVault.sol";

/// @notice Binds a device key to an ERC-8004 agent NFT owned by a person.
/// The device key is never the NFT owner and this contract never approves it
/// as an operator. The human owner is always read from `ownerOf`.
contract KineticRegistry is EIP712, ReentrancyGuard, IERC721Receiver {
    bytes32 private constant CLAIM_TYPEHASH = keccak256("ClaimTicket(address device,address owner,uint256 nonce)");

    struct Limits {
        uint256 perTxCap;
        uint256 dailyCap;
    }

    error ZeroAddress();
    error VaultAlreadySet();
    error VaultUnset();
    error NotDeployer();
    error NotOwner();
    error DeviceIsOwner();
    error BadSignature();
    error AlreadyBound();
    error NotBound();
    error NotIdentity();

    event VaultSet(address indexed vault);
    event Claimed(uint256 indexed agentId, address indexed owner, address indexed device);
    event Linked(uint256 indexed agentId, address indexed owner, address indexed device);
    event Rotated(uint256 indexed agentId, address indexed previousDevice, address indexed device);
    event Revoked(uint256 indexed agentId, address indexed device);

    IERC8004Identity public immutable identity;
    address public immutable deployer;

    address public vault;

    mapping(address device => uint256 nonce) public claimNonce;
    mapping(address device => uint256 agentId) private _deviceAgent;
    mapping(address device => bool bound) private _deviceBound;
    mapping(uint256 agentId => address device) public agentDevice;

    constructor(address identityRegistry) EIP712("KineticRegistry", "1") {
        if (identityRegistry == address(0)) revert ZeroAddress();
        identity = IERC8004Identity(identityRegistry);
        deployer = msg.sender;
    }

    /// @notice One-shot wiring. The deployer has no other authority.
    function setVault(address vault_) external {
        if (msg.sender != deployer) revert NotDeployer();
        if (vault != address(0) || vault_ == address(0)) revert VaultAlreadySet();
        vault = vault_;
        emit VaultSet(vault_);
    }

    /// @notice Mint an agent NFT to the caller and bind `device` to it.
    /// `signature` is the device's EIP-712 claim ticket naming this owner.
    function claim(address device, bytes calldata signature, string calldata agentURI, Limits calldata limits)
        external
        nonReentrant
        returns (uint256 agentId)
    {
        if (vault == address(0)) revert VaultUnset();
        agentId = identity.register(agentURI);
        identity.transferFrom(address(this), msg.sender, agentId);
        _bind(agentId, device, signature, limits);
        emit Claimed(agentId, msg.sender, device);
    }

    /// @notice Bind `device` to an agent NFT the caller already owns.
    function link(uint256 agentId, address device, bytes calldata signature, Limits calldata limits)
        external
        nonReentrant
    {
        if (vault == address(0)) revert VaultUnset();
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        _bind(agentId, device, signature, limits);
        emit Linked(agentId, msg.sender, device);
    }

    /// @notice Replace the device key. The new device signs a claim ticket.
    function rotate(uint256 agentId, address newDevice, bytes calldata signature) external nonReentrant {
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        address previous = agentDevice[agentId];
        if (previous == address(0)) revert NotBound();
        if (newDevice == address(0) || newDevice == msg.sender) revert DeviceIsOwner();
        if (_deviceBound[newDevice]) revert AlreadyBound();

        _consumeClaim(newDevice, msg.sender, signature);
        _clearDevice(previous);
        _deviceBound[newDevice] = true;
        _deviceAgent[newDevice] = agentId;
        agentDevice[agentId] = newDevice;
        emit Rotated(agentId, previous, newDevice);
    }

    /// @notice Drop the device key and pause the vault. The NFT stays with the owner.
    function revoke(uint256 agentId) external nonReentrant {
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        address device = agentDevice[agentId];
        if (device == address(0)) revert NotBound();
        _clearDevice(device);
        delete agentDevice[agentId];
        IDeviceVault(vault).pauseFromRegistry(agentId);
        emit Revoked(agentId, device);
    }

    /// @return agentId Meaningful only when `bound` is true. Agent ids can be zero.
    /// @return bound False when this device has no agent.
    function deviceToAgent(address device) external view returns (uint256 agentId, bool bound) {
        return (_deviceAgent[device], _deviceBound[device]);
    }

    /// @notice Live ERC-721 owner. This contract does not cache it.
    function ownerOfAgent(uint256 agentId) external view returns (address) {
        return identity.ownerOf(agentId);
    }

    function hashClaim(address device, address owner_, uint256 nonce) external view returns (bytes32) {
        return _hashTypedDataV4(keccak256(abi.encode(CLAIM_TYPEHASH, device, owner_, nonce)));
    }

    /// @notice Accept the agent NFT only from the Identity Registry, during `register`.
    function onERC721Received(address, address, uint256, bytes calldata) external view returns (bytes4) {
        if (msg.sender != address(identity)) revert NotIdentity();
        return IERC721Receiver.onERC721Received.selector;
    }

    function _bind(uint256 agentId, address device, bytes calldata signature, Limits calldata limits) private {
        if (device == address(0) || device == msg.sender) revert DeviceIsOwner();
        if (agentDevice[agentId] != address(0) || _deviceBound[device]) revert AlreadyBound();
        _consumeClaim(device, msg.sender, signature);
        _deviceBound[device] = true;
        _deviceAgent[device] = agentId;
        agentDevice[agentId] = device;
        IDeviceVault(vault).initialize(agentId, limits.perTxCap, limits.dailyCap);
    }

    function _consumeClaim(address device, address owner_, bytes calldata signature) private {
        uint256 nonce = claimNonce[device];
        bytes32 digest = _hashTypedDataV4(keccak256(abi.encode(CLAIM_TYPEHASH, device, owner_, nonce)));
        (address signer, ECDSA.RecoverError err,) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || signer != device) revert BadSignature();
        claimNonce[device] = nonce + 1;
    }

    function _clearDevice(address device) private {
        _deviceBound[device] = false;
        delete _deviceAgent[device];
    }
}
