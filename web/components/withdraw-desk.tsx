"use client";

import { motion, useReducedMotion } from "framer-motion";
import Link from "next/link";
import { useState } from "react";
import { createPublicClient, http, isAddress, parseEther, type Address, type Hash } from "viem";
import { useChainId, useConnect, useConnection, useConnectors, useDisconnect, useSwitchChain, useWriteContract, type Connector } from "wagmi";

import { Atmosphere } from "@/components/storm";
import { WalletPill } from "@/components/wallet-pill";
import { homeHref } from "@/lib/links";
import { explorerTx, KINETIC, monadTestnet } from "@/lib/monad";

const CHAIN_ID = 10143;
const ZERO_ADDRESS = "0x0000000000000000000000000000000000000000";

const registryAbi = [
  {
    type: "function",
    name: "deviceToAgent",
    stateMutability: "view",
    inputs: [{ name: "device", type: "address" }],
    outputs: [
      { name: "agentId", type: "uint256" },
      { name: "bound", type: "bool" },
    ],
  },
  {
    type: "function",
    name: "agentDevice",
    stateMutability: "view",
    inputs: [{ name: "agentId", type: "uint256" }],
    outputs: [{ name: "device", type: "address" }],
  },
  {
    type: "function",
    name: "ownerOfAgent",
    stateMutability: "view",
    inputs: [{ name: "agentId", type: "uint256" }],
    outputs: [{ name: "owner", type: "address" }],
  },
] as const;

const vaultAbi = [
  {
    type: "function",
    name: "withdraw",
    stateMutability: "nonpayable",
    inputs: [
      { name: "agentId", type: "uint256" },
      { name: "amount", type: "uint256" },
    ],
    outputs: [],
  },
] as const;

const RISE = { type: "spring" as const, stiffness: 320, damping: 30 };

export function WithdrawDesk({
  host,
  address,
  agentId,
}: {
  host: string;
  address: string;
  agentId: string;
}) {
  const reduce = useReducedMotion();
  const connection = useConnection();
  const chainId = useChainId();
  const connectors = useConnectors();
  const { connectAsync, isPending: connecting } = useConnect();
  const { disconnectAsync } = useDisconnect();
  const { switchChainAsync, isPending: switching } = useSwitchChain();
  const { writeContractAsync } = useWriteContract();
  const [vaultAddress, setVaultAddress] = useState(address);
  const [amount, setAmount] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hash, setHash] = useState<Hash | null>(null);
  const connected = connection.status === "connected" ? connection.address : undefined;
  const home = homeHref(host);
  const contracts = KINETIC[CHAIN_ID];
  const validAgent = isAddress(vaultAddress.trim());
  const validAmount = amountOk(amount);
  const canWithdraw = Boolean(connected && validAgent && validAmount && !busy && !switching);

  async function connectWallet(connector: Connector) {
    if (connection.status === "connected" && connection.connector.id !== connector.id) await disconnectAsync();
    await connectAsync({ connector });
  }

  async function withdraw() {
    if (!canWithdraw || !connected) return;
    setError(null);
    setHash(null);
    setBusy(true);
    try {
      if (chainId !== CHAIN_ID) await switchChainAsync({ chainId: CHAIN_ID });
      const client = createPublicClient({ chain: monadTestnet, transport: http() });
      const entered = vaultAddress.trim();
      let id: bigint;
      if (sameAddress(entered, contracts.vault)) {
        if (!/^\d+$/.test(agentId.trim())) {
          setError("This vault address is shared by every agent. Open the withdraw link from Telegram so the page knows which agent to pay.");
          return;
        }
        id = BigInt(agentId.trim());
        const device = await client.readContract({
          address: contracts.registry,
          abi: registryAbi,
          functionName: "agentDevice",
          args: [id],
        });
        if (sameAddress(device, ZERO_ADDRESS)) {
          setError("This agent is not claimed yet.");
          return;
        }
      } else {
        const found = await client.readContract({
          address: contracts.registry,
          abi: registryAbi,
          functionName: "deviceToAgent",
          args: [entered as Address],
        });
        if (!found[1]) {
          setError("This agent is not claimed yet.");
          return;
        }
        id = found[0];
      }
      const owner = await client.readContract({
        address: contracts.registry,
        abi: registryAbi,
        functionName: "ownerOfAgent",
        args: [id],
      });
      if (!sameAddress(owner, connected)) {
        setError("Connect the owner wallet to withdraw.");
        return;
      }
      const tx = await writeContractAsync({
        address: contracts.vault,
        abi: vaultAbi,
        functionName: "withdraw",
        chainId: CHAIN_ID,
        args: [id, parseEther(amount.trim())],
      });
      setHash(tx);
    } catch (cause) {
      setError(explain(cause));
    } finally {
      setBusy(false);
    }
  }

  const explorer = hash ? explorerTx(CHAIN_ID, hash) : null;

  return (
    <div className="relative min-h-dvh">
      <Atmosphere />
      <header className="absolute inset-x-0 top-0 z-10 flex items-center justify-between px-5 py-4 sm:px-10">
        {home.startsWith("/") ? (
          <Link href={home} className="brand">
            KINETIC
          </Link>
        ) : (
          <a href={home} className="brand">
            KINETIC
          </a>
        )}
        <WalletPill
          address={connected}
          connectors={connectors}
          currentId={connection.status === "connected" ? connection.connector.id : undefined}
          connecting={connecting}
          locked={busy || switching}
          onConnect={connectWallet}
          onDisconnect={disconnectAsync}
        />
      </header>
      <main className="flex min-h-dvh items-center justify-center px-6 pb-16 pt-24">
        <motion.section
          className={`w-full max-w-lg border bg-raise px-6 py-8 backdrop-blur-md sm:px-8 ${
            hash ? "claim-done" : error ? "claim-failed" : "border-line"
          }`}
          initial={reduce ? false : { opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          transition={RISE}
        >
          <p className="font-mono text-xs tracking-[0.28em] text-violet">WITHDRAW</p>
          <h1 className="mt-3 text-3xl">{"Withdraw from this Agent's Vault"}</h1>
          <p className="mt-3 text-sm leading-7 text-dim">
            Enter the agent vault address and an amount. The owner wallet receives that amount from the vault.
          </p>
          <form
            className="mt-8 flex flex-col gap-5"
            onSubmit={(event) => {
              event.preventDefault();
              void withdraw();
            }}
          >
            <label className="flex flex-col gap-2 text-sm text-dim">
              Agent Vault Address
              <input
                className="field"
                name="address"
                autoComplete="off"
                spellCheck={false}
                value={vaultAddress}
                onChange={(event) => setVaultAddress(event.target.value)}
                placeholder="0x"
              />
            </label>
            <label className="flex flex-col gap-2 text-sm text-dim">
              Amount
              <input
                className="field"
                name="amount"
                inputMode="decimal"
                autoComplete="off"
                value={amount}
                onChange={(event) => setAmount(event.target.value)}
                placeholder="1"
              />
            </label>
            <button type="submit" className="btn-white w-full" disabled={!canWithdraw}>
              {busy || switching ? "Confirm in the wallet" : "Withdraw"}
            </button>
          </form>
          {!connected ? (
            <p className="mt-4 text-sm leading-6 text-dim">Connect the owner wallet in the corner.</p>
          ) : null}
          {error ? (
            <p className="claim-alert mt-4 text-sm leading-6" role="alert">
              {error}
            </p>
          ) : null}
          {hash ? (
            <p className="mt-4 text-sm leading-6 text-success">
              Withdrawn.
              {explorer ? (
                <>
                  {" "}
                  <a
                    href={explorer}
                    className="font-medium text-success focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-violet"
                  >
                    View the withdrawal on the explorer
                  </a>
                </>
              ) : null}
            </p>
          ) : null}
        </motion.section>
      </main>
    </div>
  );
}

function sameAddress(left: string, right: string) {
  return left.trim().toLowerCase() === right.trim().toLowerCase();
}

function amountOk(value: string) {
  const trimmed = value.trim();
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return false;
  try {
    return parseEther(trimmed) > 0n;
  } catch {
    return false;
  }
}

function explain(cause: unknown) {
  const message =
    cause && typeof cause === "object" && "shortMessage" in cause
      ? String((cause as { shortMessage: unknown }).shortMessage)
      : cause instanceof Error
        ? cause.message
        : "The wallet rejected the request.";
  if (/provider not found/i.test(message)) {
    return "No wallet found in this browser. Install one that can add Monad, then reload.";
  }
  return message.slice(0, 220);
}
