import { useEffect } from "react";
import { connect, getAccount } from "wagmi/actions";
import { devnetConnector, wagmiConfig } from "./config";

export function DevnetWalletConnector() {
  useEffect(() => {
    if (devnetConnector && getAccount(wagmiConfig).status === "disconnected") {
      void connect(wagmiConfig, { connector: devnetConnector });
    }
  }, []);
  return null;
}
