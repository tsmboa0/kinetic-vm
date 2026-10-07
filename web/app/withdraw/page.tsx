import { headers } from "next/headers";

import { Providers } from "@/components/providers";
import { WithdrawDesk } from "@/components/withdraw-desk";

export default async function WithdrawPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const query = await searchParams;
  const headerList = await headers();
  const host = headerList.get("host")?.split(":")[0] ?? "";
  const raw = query.address;
  const address = Array.isArray(raw) ? raw[0] : raw;
  const agent = query.agent;
  const agentId = Array.isArray(agent) ? agent[0] : agent;
  return (
    <Providers>
      <WithdrawDesk host={host} address={address ?? ""} agentId={agentId ?? ""} />
    </Providers>
  );
}
