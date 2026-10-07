// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {Monad8004} from "../src/Monad8004.sol";

contract DeploymentFileTest is Test {
    function test_deployment_file_names_the_monad_testnet_registries() public view {
        string memory json = vm.readFile("deployments/10143.json");
        assertEq(vm.parseJsonUint(json, ".chainId"), 10143);
        assertEq(vm.parseJsonAddress(json, ".identityRegistry"), Monad8004.TESTNET_IDENTITY);
        assertEq(vm.parseJsonAddress(json, ".reputationRegistry"), Monad8004.TESTNET_REPUTATION);
        assertEq(vm.parseJsonAddress(json, ".validationRegistry"), Monad8004.TESTNET_VALIDATION);
        assertEq(vm.parseJsonAddress(json, ".kineticRegistry"), 0xa361931269F7e0b1957175a48669F14735E61EDA);
        assertEq(vm.parseJsonAddress(json, ".deviceVault"), 0x77102fCAC7927bB64507DF82bf1ae659465B4EE5);
        assertEq(vm.parseJsonAddress(json, ".kineticAttestor"), 0x6e46d149b7d3396b42E874765Cf5a1bb26BD5bA5);
    }
}
