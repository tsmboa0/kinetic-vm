// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {IDeviceVault} from "./interfaces/IDeviceVault.sol";
import {KineticRegistry} from "./KineticRegistry.sol";

/// @notice Per-agent native-token vault. The device can pay only an allowlisted
/// recipient, and only inside the per-transaction and daily caps. The owner
/// tightens those caps directly. Raising a cap, adding a recipient, or
/// unpausing requires the owner themselves or their EIP-712 signature.
contract DeviceVault is IDeviceVault, EIP712, ReentrancyGuard {
    bytes32 private constant LOOSEN_TYPEHASH =
        keccak256("LoosenCaps(uint256 agentId,uint256 perTxCap,uint256 dailyCap,uint256 nonce,uint256 deadline)");
    bytes32 private constant ALLOW_TYPEHASH =
        keccak256("AllowRecipient(uint256 agentId,address recipient,uint256 nonce,uint256 deadline)");
    bytes32 private constant UNPAUSE_TYPEHASH = keccak256("Unpause(uint256 agentId,uint256 nonce,uint256 deadline)");

    struct Limits {
        uint256 perTxCap;
        uint256 dailyCap;
        bool paused;
    }

    error ZeroAddress();
    error NotRegistry();
    error NotOwner();
    error NotDevice();
    error UnknownAgent();
    error BadSignature();
    error Expired();
    error NotALoosen();
    error NotATighten();
    error Paused();
    error NotPaused();
    error ZeroAmount();
    error PerTxCap();
    error DailyCap();
    error NotAllowlisted();
    error InsufficientBalance();
    error TransferFailed();
    error AlreadyAllowed();
    error NotAllowed();
    error GasCap();

    /// @notice A device-initiated gas top-up never leaves the device holding more than this.
    uint256 public constant MAX_DEVICE_GAS = 5 ether;

    event Initialized(uint256 indexed agentId, uint256 perTxCap, uint256 dailyCap);
    event Rebound(uint256 indexed agentId, uint256 perTxCap, uint256 dailyCap);
    event Deposited(uint256 indexed agentId, address indexed from, uint256 amount);
    event Paid(uint256 indexed agentId, address indexed recipient, uint256 amount);
    event GasToppedUp(uint256 indexed agentId, address indexed device, uint256 amount);
    event Withdrawn(uint256 indexed agentId, address indexed owner, uint256 amount);
    event CapsTightened(uint256 indexed agentId, uint256 perTxCap, uint256 dailyCap);
    event CapsLoosened(uint256 indexed agentId, uint256 perTxCap, uint256 dailyCap);
    event RecipientAllowed(uint256 indexed agentId, address indexed recipient);
    event RecipientDisallowed(uint256 indexed agentId, address indexed recipient);
    event SpendingPaused(uint256 indexed agentId);
    event SpendingUnpaused(uint256 indexed agentId);

    KineticRegistry public immutable registry;

    mapping(uint256 agentId => bool initialized) private _initialized;
    mapping(uint256 agentId => Limits limits) private _limits;
    mapping(uint256 agentId => mapping(address recipient => bool allowed)) private _allowed;
    mapping(uint256 agentId => address[] recipients) private _recipientList;
    mapping(uint256 agentId => SpendWindow window) private _window;

    struct SpendWindow {
        uint256 start;
        uint256 spent;
    }
    mapping(uint256 agentId => uint256 balance) private _balances;
    mapping(uint256 agentId => uint256 nonce) public loosenNonce;

    constructor(address registry_) EIP712("DeviceVault", "1") {
        if (registry_ == address(0)) revert ZeroAddress();
        registry = KineticRegistry(registry_);
    }

    function bindLimits(uint256 agentId, uint256 perTxCap, uint256 dailyCap) external {
        if (msg.sender != address(registry)) revert NotRegistry();
        if (!_initialized[agentId]) {
            _initialized[agentId] = true;
            _limits[agentId] = Limits({perTxCap: perTxCap, dailyCap: dailyCap, paused: false});
            emit Initialized(agentId, perTxCap, dailyCap);
            return;
        }
        _clearAllowlist(agentId);
        delete _window[agentId];
        _limits[agentId] = Limits({perTxCap: perTxCap, dailyCap: dailyCap, paused: true});
        emit Rebound(agentId, perTxCap, dailyCap);
    }

    function pauseFromRegistry(uint256 agentId) external {
        if (msg.sender != address(registry)) revert NotRegistry();
        if (!_initialized[agentId]) revert UnknownAgent();
        _limits[agentId].paused = true;
        emit SpendingPaused(agentId);
    }

    function deposit(uint256 agentId) external payable {
        if (!_initialized[agentId]) revert UnknownAgent();
        if (msg.value == 0) revert ZeroAmount();
        _balances[agentId] += msg.value;
        emit Deposited(agentId, msg.sender, msg.value);
    }

    /// @notice Device-only payment. This is the only path that sends value to a third party.
    function pay(uint256 agentId, address recipient, uint256 amount) external nonReentrant {
        if (msg.sender != registry.activeDevice(agentId)) revert NotDevice();
        if (recipient == address(0)) revert ZeroAddress();
        _spend(agentId, recipient, amount, true);
        emit Paid(agentId, recipient, amount);
    }

    /// @notice Send gas to the active device. The owner or that device may call it.
    /// The transfer still counts against the caps. A device call cannot leave the key above MAX_DEVICE_GAS.
    function topUpGas(uint256 agentId, uint256 amount) external nonReentrant {
        address device = registry.activeDevice(agentId);
        bool deviceCaller = msg.sender == device;
        if (msg.sender != _owner(agentId) && !deviceCaller) revert NotOwner();
        if (deviceCaller) {
            uint256 balance = device.balance;
            if (balance >= MAX_DEVICE_GAS || amount > MAX_DEVICE_GAS - balance) revert GasCap();
        }
        _spend(agentId, device, amount, false);
        emit GasToppedUp(agentId, device, amount);
    }

    /// @notice Owner recovery. Ignores the device caps. Works while paused.
    function withdraw(uint256 agentId, uint256 amount) external nonReentrant {
        address owner = _owner(agentId);
        if (msg.sender != owner) revert NotOwner();
        if (!_initialized[agentId]) revert UnknownAgent();
        if (amount == 0) revert ZeroAmount();
        if (_balances[agentId] < amount) revert InsufficientBalance();
        _balances[agentId] -= amount;
        (bool ok,) = owner.call{value: amount}("");
        if (!ok) revert TransferFailed();
        emit Withdrawn(agentId, owner, amount);
    }

    function tightenCaps(uint256 agentId, uint256 perTxCap, uint256 dailyCap) external {
        if (msg.sender != _owner(agentId)) revert NotOwner();
        Limits storage limits = _existing(agentId);
        if (perTxCap > limits.perTxCap || dailyCap > limits.dailyCap) revert NotATighten();
        if (perTxCap == limits.perTxCap && dailyCap == limits.dailyCap) revert NotATighten();
        limits.perTxCap = perTxCap;
        limits.dailyCap = dailyCap;
        emit CapsTightened(agentId, perTxCap, dailyCap);
    }

    /// @notice Raise caps. The owner may send this directly. Anyone else needs the owner's signature.
    function loosenCaps(uint256 agentId, uint256 perTxCap, uint256 dailyCap, uint256 deadline, bytes calldata signature)
        external
    {
        Limits storage limits = _existing(agentId);
        if (perTxCap < limits.perTxCap || dailyCap < limits.dailyCap) revert NotALoosen();
        if (perTxCap == limits.perTxCap && dailyCap == limits.dailyCap) revert NotALoosen();
        _authorizeLoosen(
            agentId,
            keccak256(abi.encode(LOOSEN_TYPEHASH, agentId, perTxCap, dailyCap, loosenNonce[agentId], deadline)),
            deadline,
            signature
        );
        limits.perTxCap = perTxCap;
        limits.dailyCap = dailyCap;
        emit CapsLoosened(agentId, perTxCap, dailyCap);
    }

    function allowRecipient(uint256 agentId, address recipient, uint256 deadline, bytes calldata signature) external {
        if (recipient == address(0)) revert ZeroAddress();
        _existing(agentId);
        if (_allowed[agentId][recipient]) revert AlreadyAllowed();
        _authorizeLoosen(
            agentId,
            keccak256(abi.encode(ALLOW_TYPEHASH, agentId, recipient, loosenNonce[agentId], deadline)),
            deadline,
            signature
        );
        _allowed[agentId][recipient] = true;
        _recipientList[agentId].push(recipient);
        emit RecipientAllowed(agentId, recipient);
    }

    function disallowRecipient(uint256 agentId, address recipient) external {
        if (msg.sender != _owner(agentId)) revert NotOwner();
        _existing(agentId);
        if (!_allowed[agentId][recipient]) revert NotAllowed();
        _allowed[agentId][recipient] = false;
        _removeRecipient(agentId, recipient);
        emit RecipientDisallowed(agentId, recipient);
    }

    function pause(uint256 agentId) external {
        if (msg.sender != _owner(agentId)) revert NotOwner();
        Limits storage limits = _existing(agentId);
        if (limits.paused) revert Paused();
        limits.paused = true;
        emit SpendingPaused(agentId);
    }

    function unpause(uint256 agentId, uint256 deadline, bytes calldata signature) external {
        Limits storage limits = _existing(agentId);
        if (!limits.paused) revert NotPaused();
        _authorizeLoosen(
            agentId,
            keccak256(abi.encode(UNPAUSE_TYPEHASH, agentId, loosenNonce[agentId], deadline)),
            deadline,
            signature
        );
        limits.paused = false;
        emit SpendingUnpaused(agentId);
    }

    function limitsOf(uint256 agentId) external view returns (uint256 perTxCap, uint256 dailyCap, bool paused) {
        Limits storage limits = _existing(agentId);
        return (limits.perTxCap, limits.dailyCap, limits.paused);
    }

    function balanceOf(uint256 agentId) external view returns (uint256) {
        return _balances[agentId];
    }

    /// @notice Amount spent in the current 24-hour window. Zero once that window has elapsed.
    function spentToday(uint256 agentId) external view returns (uint256) {
        SpendWindow storage window = _window[agentId];
        if (window.start == 0 || block.timestamp >= window.start + 1 days) return 0;
        return window.spent;
    }

    function isAllowed(uint256 agentId, address recipient) external view returns (bool) {
        return _allowed[agentId][recipient];
    }

    function hashLoosenCaps(uint256 agentId, uint256 perTxCap, uint256 dailyCap, uint256 nonce, uint256 deadline)
        external
        view
        returns (bytes32)
    {
        return _hashTypedDataV4(keccak256(abi.encode(LOOSEN_TYPEHASH, agentId, perTxCap, dailyCap, nonce, deadline)));
    }

    function hashAllowRecipient(uint256 agentId, address recipient, uint256 nonce, uint256 deadline)
        external
        view
        returns (bytes32)
    {
        return _hashTypedDataV4(keccak256(abi.encode(ALLOW_TYPEHASH, agentId, recipient, nonce, deadline)));
    }

    function hashUnpause(uint256 agentId, uint256 nonce, uint256 deadline) external view returns (bytes32) {
        return _hashTypedDataV4(keccak256(abi.encode(UNPAUSE_TYPEHASH, agentId, nonce, deadline)));
    }

    function _spend(uint256 agentId, address recipient, uint256 amount, bool checkAllowlist) private {
        Limits storage limits = _existing(agentId);
        if (limits.paused) revert Paused();
        if (amount == 0) revert ZeroAmount();
        if (checkAllowlist && !_allowed[agentId][recipient]) revert NotAllowlisted();
        if (amount > limits.perTxCap) revert PerTxCap();
        SpendWindow storage window = _window[agentId];
        if (window.start == 0 || block.timestamp >= window.start + 1 days) {
            window.start = block.timestamp;
            window.spent = 0;
        }
        uint256 nextSpent = window.spent + amount;
        if (nextSpent > limits.dailyCap) revert DailyCap();
        if (_balances[agentId] < amount) revert InsufficientBalance();
        window.spent = nextSpent;
        _balances[agentId] -= amount;
        (bool ok,) = recipient.call{value: amount}("");
        if (!ok) revert TransferFailed();
    }

    /// @dev A direct call from the owner is the wallet signature. Any other caller must present one.
    function _authorizeLoosen(uint256 agentId, bytes32 structHash, uint256 deadline, bytes calldata signature) private {
        address owner = _owner(agentId);
        if (msg.sender == owner) return;
        if (block.timestamp > deadline) revert Expired();
        bytes32 digest = _hashTypedDataV4(structHash);
        (address signer, ECDSA.RecoverError err,) = ECDSA.tryRecover(digest, signature);
        if (err != ECDSA.RecoverError.NoError || signer != owner) revert BadSignature();
        loosenNonce[agentId] += 1;
    }

    function _existing(uint256 agentId) private view returns (Limits storage limits) {
        if (!_initialized[agentId]) revert UnknownAgent();
        return _limits[agentId];
    }

    function _owner(uint256 agentId) private view returns (address) {
        return registry.ownerOfAgent(agentId);
    }

    function _clearAllowlist(uint256 agentId) private {
        address[] storage list = _recipientList[agentId];
        for (uint256 i; i < list.length; ++i) {
            _allowed[agentId][list[i]] = false;
        }
        delete _recipientList[agentId];
    }

    function _removeRecipient(uint256 agentId, address recipient) private {
        address[] storage list = _recipientList[agentId];
        uint256 length = list.length;
        for (uint256 i; i < length; ++i) {
            if (list[i] != recipient) continue;
            list[i] = list[length - 1];
            list.pop();
            return;
        }
    }

    receive() external payable {
        revert ZeroAmount();
    }
}
