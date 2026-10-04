// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice The slice of the ERC-8004 Identity Registry this project calls.
/// `register` mints the agent NFT to `msg.sender`.
interface IERC8004Identity {
    function register(string calldata agentURI) external returns (uint256 agentId);

    function ownerOf(uint256 tokenId) external view returns (address);

    function transferFrom(address from, address to, uint256 tokenId) external;

    function getApproved(uint256 tokenId) external view returns (address);

    function isApprovedForAll(address owner, address operator) external view returns (bool);
}

/// @notice The slice of the ERC-8004 Validation Registry this project calls.
/// `validationRequest` succeeds only for the agent owner or an operator.
/// `validationResponse` succeeds only for the validator named in that request.
interface IERC8004Validation {
    function validationRequest(
        address validatorAddress,
        uint256 agentId,
        string calldata requestURI,
        bytes32 requestHash
    ) external;

    function validationResponse(
        bytes32 requestHash,
        uint8 response,
        string calldata responseURI,
        bytes32 responseHash,
        string calldata tag
    ) external;
}
