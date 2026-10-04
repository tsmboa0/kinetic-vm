// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {ERC721} from "@openzeppelin/contracts/token/ERC721/ERC721.sol";

/// @notice Identity Registry stand-in. `register` mints to the caller, matching ERC-8004.
contract MockIdentityRegistry is ERC721 {
    uint256 public nextId;

    constructor() ERC721("AgentIdentity", "AGENT") {}

    function register(string calldata) external returns (uint256 agentId) {
        agentId = nextId++;
        _safeMint(msg.sender, agentId);
    }
}

/// @notice Validation Registry stand-in with the same caller rules as ERC-8004.
contract MockValidationRegistry {
    MockIdentityRegistry public immutable identity;

    mapping(bytes32 requestHash => address validator) public validatorOf;
    mapping(bytes32 requestHash => uint8 response) public responseOf;
    mapping(bytes32 requestHash => bytes32 responseHash) public responseHashOf;
    mapping(bytes32 requestHash => string tag) public tagOf;
    mapping(bytes32 requestHash => bool responded) public responded;

    event ValidationRequest(
        address indexed validatorAddress, uint256 indexed agentId, string requestURI, bytes32 indexed requestHash
    );
    event ValidationResponse(
        address indexed validatorAddress,
        uint256 indexed agentId,
        bytes32 indexed requestHash,
        uint8 response,
        string responseURI,
        bytes32 responseHash,
        string tag
    );

    constructor(address identityRegistry) {
        identity = MockIdentityRegistry(identityRegistry);
    }

    function validationRequest(
        address validatorAddress,
        uint256 agentId,
        string calldata requestURI,
        bytes32 requestHash
    ) external {
        require(validatorAddress != address(0), "bad validator");
        require(validatorOf[requestHash] == address(0), "exists");
        address owner = identity.ownerOf(agentId);
        require(
            msg.sender == owner || identity.isApprovedForAll(owner, msg.sender)
                || identity.getApproved(agentId) == msg.sender,
            "Not authorized"
        );
        validatorOf[requestHash] = validatorAddress;
        emit ValidationRequest(validatorAddress, agentId, requestURI, requestHash);
    }

    function validationResponse(
        bytes32 requestHash,
        uint8 response,
        string calldata responseURI,
        bytes32 responseHash,
        string calldata tag
    ) external {
        address validator = validatorOf[requestHash];
        require(validator != address(0), "unknown");
        require(msg.sender == validator, "not validator");
        require(response <= 100, "resp>100");
        responseOf[requestHash] = response;
        responseHashOf[requestHash] = responseHash;
        tagOf[requestHash] = tag;
        responded[requestHash] = true;
        emit ValidationResponse(validator, 0, requestHash, response, responseURI, responseHash, tag);
    }
}
