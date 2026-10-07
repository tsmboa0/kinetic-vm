import { headers } from "next/headers";

import { Landing } from "@/components/landing";

export default async function Page() {
  const headerList = await headers();
  const host = headerList.get("host")?.split(":")[0] ?? "";
  return <Landing host={host} />;
}
