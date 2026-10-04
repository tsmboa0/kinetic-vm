// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {KineticRegistry} from "../src/KineticRegistry.sol";
import {DeviceVault} from "../src/DeviceVault.sol";
import {KineticAttestor} from "../src/KineticAttestor.sol";
import {Monad8004} from "../src/Monad8004.sol";

/// @notice Deploy the three KineticVM contracts to Monad testnet (chain 10143).
///
/// ```
/// MONAD_DEPLOYER_KEY=0x... forge script script/Deploy.s.sol \
///   --rpc-url "$MONAD_RPC_URL" --broadcast
/// WRITE_DEPLOYMENT=true forge script script/Deploy.s.sol --rpc-url "$MONAD_RPC_URL" --broadcast
/// ```
///
/// `WRITE_DEPLOYMENT=true` rewrites `deployments/10143.json` with the deployed addresses.
/// The key stays in the environment. Do not commit it.
contract Deploy is Script {
    error UnsupportedChain();

    function run() external {
        if (block.chainid != 10143) revert UnsupportedChain();
        uint256 key = vm.envUint("MONAD_DEPLOYER_KEY");

        vm.startBroadcast(key);
        KineticRegistry registry = new KineticRegistry(Monad8004.TESTNET_IDENTITY);
        DeviceVault vault = new DeviceVault(address(registry));
        registry.setVault(address(vault));
        KineticAttestor attestor = new KineticAttestor(address(registry), Monad8004.TESTNET_VALIDATION);
        vm.stopBroadcast();

        if (vm.envOr("WRITE_DEPLOYMENT", false)) {
            string memory json = string.concat(
                "{\n",
                '  "chainId": 10143,\n',
                '  "identityRegistry": "',
                vm.toString(Monad8004.TESTNET_IDENTITY),
                '",\n',
                '  "reputationRegistry": "',
                vm.toString(Monad8004.TESTNET_REPUTATION),
                '",\n',
                '  "validationRegistry": "',
                vm.toString(Monad8004.TESTNET_VALIDATION),
                '",\n',
                '  "kineticRegistry": "',
                vm.toString(address(registry)),
                '",\n',
                '  "deviceVault": "',
                vm.toString(address(vault)),
                '",\n',
                '  "kineticAttestor": "',
                vm.toString(address(attestor)),
                '"\n',
                "}\n"
            );
            vm.writeFile("deployments/10143.json", json);
        }
    }
}
