import { getAddress, isAddress, parseEther, type Address, type Hex } from "viem";

export type ClaimPayload = {
  device: Address;
  owner: Address;
  signature: Hex;
  deadline: bigint;
  perTx: string;
  daily: string;
  perTxWei: bigint;
  dailyWei: bigint;
  chainId: number;
  name: string;
  registry: Address;
  vault: Address;
  attestor: Address;
  rpc: string;
};

export type ClaimParse = { ok: true; value: ClaimPayload } | { ok: false; problems: string[] };

const REQUIRED = [
  "device",
  "owner",
  "sig",
  "deadline",
  "perTx",
  "daily",
  "chainId",
  "registry",
  "vault",
  "attestor",
  "rpc",
] as const;

/** Published KineticVM contracts. Same addresses as contracts/deployments/10143.json. */
const TESTNET_KINETIC = {
  registry: "0xBf2E634F8DA4C8C02979C1A2CcAD113eFb259132",
  vault: "0xD01eEa46c7E054f98dde0ed58DB5Da73ED4D22fD",
  attestor: "0xAb523187C7687743B29daf3468891E339CbF8f82",
  rpc: "https://testnet-rpc.monad.xyz",
} as const;

export function parseClaim(input: Record<string, string | string[] | undefined>): ClaimParse {
  const read = (key: string) => {
    const value = input[key];
    return Array.isArray(value) ? value[0] : value;
  };
  const token = read("t")?.trim();
  if (token) return parseCompact(token);
  return parseFields(read);
}

function parseFields(read: (key: string) => string | undefined): ClaimParse {
  const problems: string[] = [];
  for (const key of REQUIRED) {
    if (!read(key)?.trim()) problems.push(`Missing ${key}.`);
  }
  if (problems.length > 0) return { ok: false, problems };

  const device = address(read("device"), "device", problems);
  const owner = address(read("owner"), "owner", problems);
  const registry = address(read("registry"), "registry", problems);
  const vault = address(read("vault"), "vault", problems);
  const attestor = address(read("attestor"), "attestor", problems);
  const signature = hexBytes(read("sig"), problems);
  const deadline = uint(read("deadline"), "deadline", problems);
  const chainId = Number(read("chainId"));
  if (!Number.isInteger(chainId) || chainId <= 0) problems.push("chainId is not a chain id.");
  const perTx = read("perTx")!.trim();
  const daily = read("daily")!.trim();
  let perTxWei = 0n;
  let dailyWei = 0n;
  try {
    perTxWei = parseEther(perTx);
    dailyWei = parseEther(daily);
    if (perTxWei > dailyWei) problems.push("The per-transaction cap is above the daily cap.");
  } catch {
    problems.push("Caps must be MON amounts such as 0.05.");
  }
  const rpc = read("rpc")!.trim();
  if (!rpc.startsWith("https://")) problems.push("rpc must be an https address.");
  if (deadline !== null && deadline <= BigInt(Math.floor(Date.now() / 1000))) {
    problems.push("This claim link has expired. Start the device again for a new one.");
  }
  if (
    problems.length > 0 ||
    !device ||
    !owner ||
    !registry ||
    !vault ||
    !attestor ||
    !signature ||
    deadline === null
  ) {
    return { ok: false, problems };
  }
  return {
    ok: true,
    value: {
      device,
      owner,
      signature,
      deadline,
      perTx,
      daily,
      perTxWei,
      dailyWei,
      chainId,
      name: read("name")?.trim() || "Kinetic device",
      registry,
      vault,
      attestor,
      rpc,
    },
  };
}

export function agentUri(name: string, device: Address): string {
  return `data:application/json,${encodeURIComponent(JSON.stringify({ name, device }))}`;
}

function parseCompact(token: string): ClaimParse {
  const problems = ["This claim link is not one this page can open."];
  const bytes = decodeBase64Url(token);
  if (!bytes) return { ok: false, problems };
  const reader = new Reader(bytes);
  try {
    if (reader.u8() !== 1) return { ok: false, problems };
    const chainId = reader.u32();
    const deadline = reader.u64();
    const device = address(hex(reader.take(20)), "device", []);
    const owner = address(hex(reader.take(20)), "owner", []);
    const signature = hex(reader.take(reader.u8())) as Hex;
    const perTx = reader.text();
    const daily = reader.text();
    const name = reader.text();
    const signatureOk =
      /^0x[0-9a-f]+$/.test(signature) && signature.length >= 4 && signature.length % 2 === 0;
    if (!reader.done() || chainId !== 10143 || !device || !owner || !signatureOk) {
      return { ok: false, problems };
    }
    let perTxWei = 0n;
    let dailyWei = 0n;
    try {
      perTxWei = parseEther(perTx);
      dailyWei = parseEther(daily);
      if (perTxWei > dailyWei) {
        return {
          ok: false,
          problems: ["The per-transaction cap is above the daily cap."],
        };
      }
    } catch {
      return { ok: false, problems: ["Caps must be MON amounts such as 0.05."] };
    }
    if (deadline <= BigInt(Math.floor(Date.now() / 1000))) {
      return {
        ok: false,
        problems: ["This claim link has expired. Start the device again for a new one."],
      };
    }
    return {
      ok: true,
      value: {
        device,
        owner,
        signature,
        deadline,
        perTx,
        daily,
        perTxWei,
        dailyWei,
        chainId,
        name: name.trim() || "Kinetic device",
        registry: getAddress(TESTNET_KINETIC.registry),
        vault: getAddress(TESTNET_KINETIC.vault),
        attestor: getAddress(TESTNET_KINETIC.attestor),
        rpc: TESTNET_KINETIC.rpc,
      },
    };
  } catch {
    return { ok: false, problems };
  }
}

class Reader {
  private at = 0;

  constructor(private readonly data: Uint8Array) {}

  done(): boolean {
    return this.at === this.data.length;
  }

  u8(): number {
    if (this.at >= this.data.length) throw new Error("short");
    return this.data[this.at++];
  }

  u32(): number {
    if (this.at + 4 > this.data.length) throw new Error("short");
    const value = new DataView(this.data.buffer, this.data.byteOffset + this.at, 4).getUint32(0);
    this.at += 4;
    return value;
  }

  u64(): bigint {
    const high = BigInt(this.u32());
    const low = BigInt(this.u32());
    return (high << 32n) + low;
  }

  take(count: number): Uint8Array {
    if (this.at + count > this.data.length) throw new Error("short");
    const slice = this.data.subarray(this.at, this.at + count);
    this.at += count;
    return slice;
  }

  text(): string {
    const count = this.u8();
    return new TextDecoder("utf-8", { fatal: true }).decode(this.take(count));
  }
}

function decodeBase64Url(token: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]+$/.test(token)) return null;
  const pad = "=".repeat((4 - (token.length % 4)) % 4);
  try {
    const binary = atob(token.replace(/-/g, "+").replace(/_/g, "/") + pad);
    const out = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) out[index] = binary.charCodeAt(index);
    return out;
  } catch {
    return null;
  }
}

function hex(bytes: Uint8Array): string {
  let out = "0x";
  for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
  return out;
}

function address(value: string | undefined, label: string, problems: string[]): Address | null {
  const raw = value?.trim() ?? "";
  if (!isAddress(raw)) {
    problems.push(`${label} is not a wallet address.`);
    return null;
  }
  return getAddress(raw);
}

function hexBytes(value: string | undefined, problems: string[]): Hex | null {
  const raw = value?.trim() ?? "";
  if (!/^0x[0-9a-fA-F]+$/.test(raw) || raw.length < 4 || raw.length % 2 !== 0) {
    problems.push("sig is not a signature.");
    return null;
  }
  return raw as Hex;
}

function uint(value: string | undefined, label: string, problems: string[]): bigint | null {
  try {
    const parsed = BigInt(value!.trim());
    if (parsed < 0n) throw new Error("negative");
    return parsed;
  } catch {
    problems.push(`${label} is not a number.`);
    return null;
  }
}
