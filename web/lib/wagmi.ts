import { createConfig, http } from "wagmi";
import { injected } from "wagmi/connectors";

import { monadMainnet, monadTestnet } from "@/lib/monad";

export const wagmiConfig = createConfig({
  chains: [monadTestnet, monadMainnet],
  connectors: [injected({ shimDisconnect: true })],
  multiInjectedProviderDiscovery: true,
  transports: {
    [monadTestnet.id]: http(monadTestnet.rpcUrls.default.http[0]),
    [monadMainnet.id]: http(monadMainnet.rpcUrls.default.http[0]),
  },
  ssr: true,
});
