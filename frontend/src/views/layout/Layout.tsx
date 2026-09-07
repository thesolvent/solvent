import { ConnectButton } from "@rainbow-me/rainbowkit";
import { Link, Outlet } from "@tanstack/react-router";

import { useRequestTokens } from "@/application/faucet";
import { useWallet } from "@/application/wallet";

const NAV = [
  { to: "/", label: "Home" },
  { to: "/swap", label: "Swap" },
  { to: "/pools", label: "Pools" },
  { to: "/makers", label: "Makers" },
  { to: "/explorer", label: "Explorer" },
] as const;

export function Layout() {
  return (
    <div className="flex min-h-full flex-col bg-white">
      <DevnetBar />
      <Header />
      <main className="flex-1">
        <Outlet />
      </main>
    </div>
  );
}

function DevnetBar() {
  const { address, isConnected } = useWallet();
  const requestTokens = useRequestTokens();
  return (
    <div className="flex flex-wrap items-center justify-center gap-3 bg-ink px-4 py-1.5">
      <span className="font-mono text-[10px] tracking-[0.2em] text-lime uppercase">
        Devnet
      </span>
      <span className="font-mono text-[11px] text-grey-400">
        Balances are test-only. Top up before resolving intents.
      </span>
      <button
        type="button"
        disabled={!isConnected || requestTokens.isPending}
        onClick={() => {
          if (address) requestTokens.mutate(address);
        }}
        className="rounded-full bg-white px-3 py-0.5 font-mono text-[11px] tracking-wide text-ink uppercase transition-opacity hover:opacity-90 disabled:opacity-40"
      >
        {requestTokens.isPending ? "Minting…" : "Get test tokens"}
      </button>
    </div>
  );
}

function Header() {
  return (
    <header className="flex items-stretch">
      <Link to="/" className="flex items-center bg-lime px-6 py-4">
        <span className="font-display text-2xl leading-none text-ink uppercase">
          Solvent
        </span>
      </Link>
      <div className="flex flex-1 items-center bg-ink px-6">
        <nav className="flex items-center gap-6">
          {NAV.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              activeOptions={{ exact: item.to === "/" }}
              className="font-mono text-xs tracking-wide uppercase transition-colors"
              activeProps={{ className: "text-white" }}
              inactiveProps={{ className: "text-grey-400 hover:text-white" }}
            >
              {item.label}
            </Link>
          ))}
        </nav>
        <div className="ml-auto flex items-center">
          <ConnectButton
            showBalance={false}
            chainStatus="none"
            accountStatus="address"
          />
        </div>
      </div>
    </header>
  );
}
