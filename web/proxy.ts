import { NextResponse, type NextRequest } from "next/server";

export function proxy(request: NextRequest) {
  const host = request.headers.get("host")?.split(":")[0] ?? "";
  const claimHost = host === "claim.kineticvm.xyz" || host.startsWith("claim.");
  const depositHost = host === "vault-deposit.kineticvm.xyz" || host.startsWith("vault-deposit.");
  if ((claimHost || depositHost) && request.nextUrl.pathname === "/") {
    const url = request.nextUrl.clone();
    url.pathname = claimHost ? "/claim" : "/deposit";
    return NextResponse.rewrite(url);
  }
  return NextResponse.next();
}

export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico).*)"],
};
