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
    bytes32 private constant CLAIM_TYPEHASH =
        keccak256("ClaimTicket(address device,address owner,uint256 nonce,uint256 deadline)");

    /// @notice A claim ticket is valid for at most one day.
    uint256 public constant MAX_CLAIM_DEADLINE = 1 days;

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
    error Expired();
    error DeadlineTooFar();
    error AlreadyBound();
    error NotBound();
    error OwnerChanged();
    error OwnerUnchanged();
    error NotIdentity();

    event VaultSet(address indexed vault);
    event Claimed(uint256 indexed agentId, address indexed owner, address indexed device);
    event Linked(uint256 indexed agentId, address indexed owner, address indexed device);
    event Rotated(uint256 indexed agentId, address indexed previousDevice, address indexed device);
    event Revoked(uint256 indexed agentId, address indexed device);
    event Released(uint256 indexed agentId, address indexed device);

    IERC8004Identity public immutable identity;
    address public immutable deployer;

    address public vault;

    mapping(address device => uint256 nonce) public claimNonce;
    mapping(address device => uint256 agentId) private _deviceAgent;
    mapping(address device => bool bound) private _deviceBound;
    mapping(uint256 agentId => address device) public agentDevice;
    /// @notice Owner recorded when the device was bound. Spending stops if `ownerOf` moves.
    mapping(uint256 agentId => address owner) public boundOwner;

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
    function claim(
        address device,
        bytes calldata signature,
        uint256 deadline,
        string calldata agentURI,
        Limits calldata limits
    ) external nonReentrant returns (uint256 agentId) {
        if (vault == address(0)) revert VaultUnset();
        agentId = identity.register(agentURI);
        identity.transferFrom(address(this), msg.sender, agentId);
        _bind(agentId, device, signature, deadline, limits);
        emit Claimed(agentId, msg.sender, device);
    }

    /// @notice Bind `device` to an agent NFT the caller already owns.
    /// After a revoke or a transfer release, this attaches a new device to the same agent.
    function link(uint256 agentId, address device, bytes calldata signature, uint256 deadline, Limits calldata limits)
        external
        nonReentrant
    {
        if (vault == address(0)) revert VaultUnset();
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        _bind(agentId, device, signature, deadline, limits);
        emit Linked(agentId, msg.sender, device);
    }

    /// @notice Replace the device key. The new device signs a claim ticket.
    function rotate(uint256 agentId, address newDevice, bytes calldata signature, uint256 deadline)
        external
        nonReentrant
    {
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        address previous = agentDevice[agentId];
        if (previous == address(0)) revert NotBound();
        if (newDevice == address(0) || newDevice == msg.sender) revert DeviceIsOwner();
        if (_deviceBound[newDevice]) revert AlreadyBound();

        _consumeClaim(newDevice, msg.sender, signature, deadline);
        _clearDevice(previous);
        _deviceBound[newDevice] = true;
        _deviceAgent[newDevice] = agentId;
        agentDevice[agentId] = newDevice;
        boundOwner[agentId] = msg.sender;
        emit Rotated(agentId, previous, newDevice);
    }

    /// @notice Drop the device key and pause the vault. The NFT stays with the owner.
    function revoke(uint256 agentId) external nonReentrant {
        if (identity.ownerOf(agentId) != msg.sender) revert NotOwner();
        address device = agentDevice[agentId];
        if (device == address(0)) revert NotBound();
        _clearDevice(device);
        delete agentDevice[agentId];
        delete boundOwner[agentId];
        IDeviceVault(vault).pauseFromRegistry(agentId);
        emit Revoked(agentId, device);
    }

    /// @notice Clear a device after the agent NFT moves to a new owner. Anyone can call it.
    function releaseTransferred(uint256 agentId) external nonReentrant {
        address device = agentDevice[agentId];
        if (device == address(0)) revert NotBound();
        if (identity.ownerOf(agentId) == boundOwner[agentId]) revert OwnerUnchanged();
        _clearDevice(device);
        delete agentDevice[agentId];
        delete boundOwner[agentId];
        IDeviceVault(vault).pauseFromRegistry(agentId);
        emit Released(agentId, device);
    }

    /// @notice Device key allowed to spend and attest. Reverts once the NFT owner changes.
    function activeDevice(uint256 agentId) external view returns (address device) {
        device = agentDevice[agentId];
        if (device == address(0)) revert NotBound();
        if (identity.ownerOf(agentId) != boundOwner[agentId]) revert OwnerChanged();
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

    function hashClaim(address device, address owner_, uint256 nonce, uint256 deadline)
        external
        view
        returns (bytes32)
    {
        return _hashTypedDataV4(keccak256(abi.encode(CLAIM_TYPEHASH, device, owner_, nonce, deadline)));
    }

    /// @notice Accept the agent NFT only from the Identity Registry, during `register`.
    function onERC721Received(address, address, uint256, bytes calldata) external view returns (bytes4) {
        if (msg.sender != address(identity)) revert NotIdentity();
        return IERC721Receiver.onERC721Received.selector;
    }

    function _bind(uint256 agentId, address device, bytes calldata signature, uint256 deadline, Limits calldata limits)
        private
    {
        if (device == address(0) || device == msg.sender) revert DeviceIsOwner();
        if (agentDevice[agentId] != address(0) || _deviceBound[device]) revert AlreadyBound();
        _consumeClaim(device, msg.sender, signature, deadline);
        _deviceBound[device] = true;
        _deviceAgent[device] = agentId;
        agentDevice[agentId] = device;
        boundOwner[agentId] = msg.sender;
        IDeviceVault(vault).bindLimits(agentId, limits.perTxCap, limits.dailyCap);
    }

    function _consumeClaim(address device, address owner_, bytes calldata signature, uint256 deadline) private {
        if (block.timestamp > deadline) revert Expired();
        if (deadline > block.timestamp + MAX_CLAIM_DEADLINE) revert DeadlineTooFar();
        uint256 nonce = claimNonce[device];
        bytes32 digest = _hashTypedDataV4(keccak256(abi.encode(CLAIM_TYPEHASH, device, owner_, nonce, deadline)));
        (address signer, ECDSA.RecoverError err,) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || signer != device) revert BadSignature();
        claimNonce[device] = nonce + 1;
    }

    function _clearDevice(address device) private {
        _deviceBound[device] = false;
        delete _deviceAgent[device];
    }
}
