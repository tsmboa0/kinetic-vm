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
        assertEq(vm.parseJsonAddress(json, ".kineticRegistry"), 0xBf2E634F8DA4C8C02979C1A2CcAD113eFb259132);
        assertEq(vm.parseJsonAddress(json, ".deviceVault"), 0xD01eEa46c7E054f98dde0ed58DB5Da73ED4D22fD);
        assertEq(vm.parseJsonAddress(json, ".kineticAttestor"), 0xAb523187C7687743B29daf3468891E339CbF8f82);
    }
}
