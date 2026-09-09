import { type FormEvent, useEffect, useRef, useState } from "react";
import { isAddress, type Address } from "viem";

import { useFaucet } from "@/services/faucet";
import styles from "./FaucetBanner.module.css";

export function FaucetBanner() {
  const [open, setOpen] = useState(false);
  const [address, setAddress] = useState("");
  const [validation, setValidation] = useState<string>();
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const faucet = useFaucet();

  useEffect(() => {
    if (!open) return;

    inputRef.current?.focus();
    const closeOnPointerDown = (event: PointerEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };

    document.addEventListener("pointerdown", closeOnPointerDown);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnPointerDown);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  function toggle() {
    if (!open) {
      setValidation(undefined);
      faucet.reset();
    }
    setOpen((current) => !current);
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const candidate = address.trim();
    if (!isAddress(candidate)) {
      setValidation("Enter a valid 0x wallet address");
      return;
    }

    setValidation(undefined);
    faucet.mutate(candidate as Address);
  }

  const status = validation
    ? validation
    : faucet.error instanceof Error
      ? faucet.error.message
      : faucet.data
        ? `Funded ${faucet.data.tokenCount} tokens${faucet.data.gasFunded ? " and gas" : ""}.`
        : undefined;

  return (
    <div className={styles.banner}>
      <span className={styles.tag}>Devnet</span>
      <span className={styles.copy}>
        Balances are test-only. Top up before resolving intents.
      </span>
      <div className={styles.anchor} ref={containerRef}>
        <button
          type="button"
          className={styles.action}
          aria-expanded={open}
          aria-controls="faucet-card"
          onClick={toggle}
        >
          Get test tokens
        </button>
        <form
          id="faucet-card"
          className={`${styles.card} ${open ? styles.cardOpen : ""}`}
          aria-label="Fund a devnet wallet"
          aria-hidden={!open}
          onSubmit={submit}
        >
          <div className={styles.field}>
            <label className={styles.label} htmlFor="faucet-address">
              Wallet address
            </label>
            <input
              ref={inputRef}
              id="faucet-address"
              className={styles.input}
              value={address}
              onChange={(event) => {
                setAddress(event.target.value);
                setValidation(undefined);
                faucet.reset();
              }}
              placeholder="Paste your 0x address"
              autoComplete="off"
              spellCheck={false}
              disabled={faucet.isPending}
            />
            {status && (
              <p
                className={
                  faucet.data && !validation ? styles.success : styles.problem
                }
                role="status"
              >
                {status}
              </p>
            )}
          </div>
          <button
            type="submit"
            className={styles.submit}
            disabled={faucet.isPending}
          >
            {faucet.isPending ? "Funding wallet…" : "Fund with all test tokens"}
          </button>
        </form>
      </div>
    </div>
  );
}
