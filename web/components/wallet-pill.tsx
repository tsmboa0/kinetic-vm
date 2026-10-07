"use client";

import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { Connector } from "wagmi";

const EMPTY = "No wallet found in this browser. Install one that can add Monad, then reload.";

export function WalletPill({
  address,
  connectors,
  currentId,
  connecting,
  locked,
  onConnect,
  onDisconnect,
}: {
  address?: string;
  connectors: readonly Connector[];
  currentId?: string;
  connecting: boolean;
  locked: boolean;
  onConnect: (connector: Connector) => Promise<void>;
  onDisconnect: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const wallets = listedWallets(connectors);
  const [wasLocked, setWasLocked] = useState(locked);
  if (locked !== wasLocked) {
    setWasLocked(locked);
    if (locked) {
      setOpen(false);
      setError(null);
    }
  }

  function close(restoreFocus: boolean) {
    setOpen(false);
    setError(null);
    if (restoreFocus) triggerRef.current?.focus();
  }

  useEffect(() => {
    if (!open) return;
    const first = menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]');
    first?.focus();
    function onPointerDown(event: PointerEvent) {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
        setError(null);
      }
    }
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  async function choose(connector: Connector) {
    if (connector.id === currentId) {
      close(true);
      return;
    }
    setError(null);
    try {
      await onConnect(connector);
      close(true);
    } catch (cause) {
      setError(explain(cause));
    }
  }

  async function disconnect() {
    setError(null);
    try {
      await onDisconnect();
      close(true);
    } catch (cause) {
      setError(explain(cause));
    }
  }

  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (items.length === 0) return;
      const next = event.key === "ArrowDown" ? index + 1 : index - 1;
      items[(next + items.length) % items.length]?.focus();
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      items[0]?.focus();
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      items[items.length - 1]?.focus();
    }
  }

  return (
    <div className="relative" ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="btn-connect"
        aria-expanded={open}
        aria-haspopup="menu"
        aria-controls={open ? menuId : undefined}
        aria-busy={connecting}
        disabled={connecting || locked}
        onClick={() => {
          if (open) close(false);
          else {
            setError(null);
            setOpen(true);
          }
        }}
      >
        {connecting ? "Connecting" : address ? short(address) : "Connect wallet"}
      </button>
      {open ? (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          aria-label={address ? "Connected wallet" : "Choose a wallet"}
          className="wallet-menu"
          onKeyDown={onMenuKeyDown}
        >
          <p className="px-3 pb-1 pt-2 font-mono text-xs tracking-[0.22em] text-violet">
            {address ? "Change wallet" : "Choose a wallet"}
          </p>
          {wallets.length === 0 ? <p className="px-3 py-3 text-sm leading-6 text-dim">{EMPTY}</p> : null}
          {wallets.map((wallet) => {
            const current = wallet.id === currentId;
            const name = wallet.id === "injected" ? "Browser wallet" : wallet.name;
            const icon = walletIcon(wallet);
            return (
              <button
                key={wallet.id}
                type="button"
                role="menuitem"
                className="wallet-item"
                aria-current={current ? "true" : undefined}
                disabled={connecting}
                onClick={() => void choose(wallet)}
              >
                <span className="wallet-name">
                  {icon ? (
                    // Wallet icons arrive as EIP-6963 data URIs, which next/image cannot optimize.
                    // eslint-disable-next-line @next/next/no-img-element
                    <img className="wallet-logo" src={icon} alt="" width={22} height={22} />
                  ) : (
                    <span className="wallet-logo wallet-logo-empty" aria-hidden="true" />
                  )}
                  <span className="min-w-0 truncate">{name}</span>
                </span>
                {current ? <span className="text-xs text-dim">Current</span> : null}
              </button>
            );
          })}
          {address ? (
            <>
              <div className="mx-3 my-1 h-px bg-line" aria-hidden="true" />
              <button
                type="button"
                role="menuitem"
                className="wallet-item text-violet-hot"
                disabled={connecting}
                onClick={() => void disconnect()}
              >
                Disconnect
              </button>
            </>
          ) : null}
          {error ? (
            <p className="px-3 py-2 text-sm leading-6 text-violet-hot" role="alert">
              {error}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function listedWallets(connectors: readonly Connector[]) {
  const named = unique(connectors.filter((item) => item.type === "injected" && item.id !== "injected"));
  if (named.length > 0) return named;
  if (!hasInjectedProvider()) return [];
  return unique(connectors.filter((item) => item.id === "injected"));
}

function unique(connectors: Connector[]) {
  const seen = new Set<string>();
  return connectors.filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
}

function hasInjectedProvider() {
  return typeof window !== "undefined" && "ethereum" in window && Boolean(window.ethereum);
}

function walletIcon(wallet: Connector) {
  // Phantom announces its icon as a data URI with a leading newline.
  const icon = wallet.icon?.trim();
  return icon?.startsWith("data:image/") ? icon : undefined;
}

function short(value: string) {
  return value.length < 12 ? value : `${value.slice(0, 6)}…${value.slice(-4)}`;
}

function explain(cause: unknown) {
  const message =
    cause && typeof cause === "object" && "shortMessage" in cause
      ? String((cause as { shortMessage: unknown }).shortMessage)
      : cause instanceof Error
        ? cause.message
        : "The wallet rejected the request.";
  if (/provider not found/i.test(message)) return EMPTY;
  return message.slice(0, 220);
}
