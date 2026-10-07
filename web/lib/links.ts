export const links = {
  docs: "https://github.com/tsmboa0/kinetic-vm#readme",
  github: "https://github.com/tsmboa0/kinetic-vm",
  x: "https://x.com/kineticvm",
} as const;

export function homeHref(host: string): string {
  if (host.startsWith("claim.")) return "https://kineticvm.xyz";
  return "/";
}

export function claimHref(host: string): string {
  if (host === "kineticvm.xyz" || host === "www.kineticvm.xyz") {
    return "https://claim.kineticvm.xyz";
  }
  return "/claim";
}
