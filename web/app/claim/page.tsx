import { headers } from "next/headers";

import { ClaimDesk } from "@/components/claim-desk";
import { Providers } from "@/components/providers";

export default async function ClaimPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const query = await searchParams;
  const headerList = await headers();
  const host = headerList.get("host")?.split(":")[0] ?? "";
  return (
    <Providers>
      <ClaimDesk query={query} host={host} />
    </Providers>
  );
}
