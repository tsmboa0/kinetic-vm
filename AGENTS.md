# AGENTS.md - KineticVM

Instructions for AI coding assistants working in this repository.

KineticVM is infrastructure that gives physical AI devices a Monad identity
(ERC-8004), an owner-controlled wallet with spending limits, and signed
onchain attestations of their actions. The agent runtime is a modified fork
of ZeroClaw (MIT OR Apache-2.0); see `NOTICE`.

## 1. Single Source Of Truth

Do not duplicate state. Before adding a struct field, config entry, schema
field, runtime cache, or parallel lookup table, identify the canonical source:

1. If the new field creates the fact, state that explicitly.
2. If the fact already exists, resolve it from that source at use time.

Onchain state (device identity, owner, vault limits, revocation) is canonical.
Local caches exist only for offline startup and the chain wins on conflict.

## 2. Safety And Keys

- Never commit secrets, private keys, mnemonics, RPC keys, or personal data.
- Device signing keys stay behind the signer interface (software key or SE050).
  Never log, print, or serialize private key material.
- Device keys must never hold ERC-721 operator rights over their own identity.
- Spending always goes through vault limits. Do not add bypass paths.
- Do not weaken allowlists, approvals, or sandboxing without stating the risk.
- Production paths propagate errors. Avoid `unwrap()` and `expect()` unless a
  documented invariant makes panic impossible.
- Do not hide unused production code behind underscores or
  `#[allow(dead_code)]`; remove it or connect it.

## 3. Working Rules

1. Read the owning module, its wiring, and adjacent tests before editing.
2. Keep one concern per commit and use conventional commit messages.
3. Do not add heavy dependencies for minor convenience or speculative features.
4. Add the smallest useful implementation and test it at the behavior boundary.
5. Report the validation commands actually run.

## 4. User-Facing Text

User-facing CLI, tool, and onboarding text uses Fluent keys through the
runtime i18n helpers in `crates/kinetic-runtime/src/i18n.rs` rather than bare
literals. Logs and tracing stay in English.

## 5. Validation

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The lean device build must keep compiling:

```bash
cargo check --no-default-features --features agent-runtime,channel-telegram,hardware,peripheral-rpi,plugins-wasm-cranelift
```
