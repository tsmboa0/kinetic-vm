import { defineChain } from "viem";

export const monadTestnet = defineChain({
  id: 10143,
  name: "Monad Testnet",
  nativeCurrency: { name: "MON", symbol: "MON", decimals: 18 },
  rpcUrls: { default: { http: ["https://testnet-rpc.monad.xyz"] } },
  blockExplorers: {
    default: { name: "Monadscan", url: "https://testnet.monadscan.com" },
  },
});

export const monadMainnet = defineChain({
  id: 143,
  name: "Monad",
  nativeCurrency: { name: "MON", symbol: "MON", decimals: 18 },
  rpcUrls: { default: { http: ["https://rpc.monad.xyz"] } },
  blockExplorers: {
    default: { name: "Monadscan", url: "https://monadscan.com" },
  },
});

/** ERC-8004 identity registry. The owner approves the attestor here. */
export const IDENTITY: Record<number, `0x${string}`> = {
  10143: "0x8004A818BFB912233c491871b3d84c89A494BD9e",
  143: "0x8004A169FB4a3325136EB29fA0ceB6D2e539a432",
};

export function explorerTx(chainId: number, hash: string): string | null {
  const chain = chainId === 10143 ? monadTestnet : chainId === 143 ? monadMainnet : null;
  if (!chain) return null;
  return `${chain.blockExplorers.default.url}/tx/${hash}`;
}

export function chainName(chainId: number): string {
  if (chainId === 10143) return monadTestnet.name;
  if (chainId === 143) return monadMainnet.name;
  return `Chain ${chainId}`;
}
