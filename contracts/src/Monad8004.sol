// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @notice Canonical ERC-8004 CREATE2 registry addresses.
/// Testnet Validation and Reputation match the addresses published for every
/// ERC-8004 testnet, including Monad chain 10143.
library Monad8004 {
    address internal constant MAINNET_IDENTITY = 0x8004A169FB4a3325136EB29fA0ceB6D2e539a432;
    address internal constant MAINNET_REPUTATION = 0x8004BAa17C55a88189AE136b182e5fdA19dE9b63;
    address internal constant MAINNET_VALIDATION = 0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58;

    address internal constant TESTNET_IDENTITY = 0x8004A818BFB912233c491871b3d84c89A494BD9e;
    address internal constant TESTNET_REPUTATION = 0x8004B663056A597Dffe9eCcC1965A193B7388713;
    address internal constant TESTNET_VALIDATION = 0x8004Cb1BF31DAf7788923b405b754f57acEB4272;
}
