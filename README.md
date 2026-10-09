# KineticVM

KineticVM gives a physical AI device a Monad identity (ERC-8004), an
owner-controlled wallet with spending limits, and a signed attestation of
what its actuators do. The owner holds the identity. The device holds a
signing key and can spend only inside the vault limits.

The agent runtime in this repository is a modified fork of
[ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) (MIT OR Apache-2.0).
ZeroClaw Labs holds the copyright in that code; see `NOTICE`, `LICENSE-MIT`,
and `LICENSE-APACHE`. This repository is not the official ZeroClaw project.

The onchain registry, vault, and attestor are the next layer. They are not
in this tree yet. What is here is the device runtime: the agent, its tools,
the channels it listens on, and the hardware adapters.

## Install

Linux (64-bit, including Raspberry Pi) and macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/tsmboa0/kinetic-vm/main/install.sh | sh
```

This puts the latest release binary in `~/.local/bin`. Then run `kinetic quickstart`.

The command works after a version tag such as `v0.8.5` has been published.
`KINETIC_VERSION=v0.8.5` pins one. Building from a clone is below.

## Build

Rust 1.96 (`rust-toolchain.toml`).

```bash
cargo build --release
```

The binary is `target/release/kinetic`.

A smaller device build, without the default gateway and channel set:

```bash
cargo build --release --no-default-features --features agent-runtime,channel-telegram,hardware,peripheral-rpi,plugins-wasm-cranelift
```

## Quick start

```bash
kinetic quickstart
kinetic agent -a <alias>
```

`quickstart` writes a working agent. It needs a terminal. Config lands at
`~/.kinetic/config.toml` unless `KINETIC_CONFIG_DIR` is set. Provider
credentials belong in the OS keyring or a `KINETIC_*` environment override,
not in a committed file.

Providers you can point an agent at: Anthropic, OpenAI, Gemini, OpenRouter,
Ollama, Hailo-Ollama, and any OpenAI-compatible endpoint.

## What the runtime does

- **Channels.** Telegram for the owner. MQTT, webhooks, and filesystem
  events for machines and SOP triggers. ACP for editor clients.
- **Tools and policy.** Default autonomy is supervised: medium-risk actions
  need approval, high-risk actions are blocked. Workspace boundaries and a
  command allowlist stay in front of the shell.
- **Hardware.** Serial peripherals, GPIO on Raspberry Pi, and firmware for
  Pico, Nucleo, Arduino, and ESP32.
- **Gateway.** HTTP and WebSocket API for clients on the device.
- **SOP engine.** Event-triggered procedures (MQTT, webhook, cron,
  peripheral) with approval gates.

## Layout

```
kinetic                 CLI and composition
kinetic-runtime         agent loop, security, SOP, daemon
kinetic-channels        Telegram, MQTT, webhook, filesystem, ACP
kinetic-gateway         HTTP / WebSocket API
kinetic-providers       model providers
kinetic-tools           shell, files, HTTP, and the other agent tools
kinetic-memory          sqlite (and optional postgres) memory
kinetic-hardware        boards, serial, firmware flashing
kinetic-config          ~/.kinetic/config.toml schema
```

## Security

Do not open a public GitHub issue for a vulnerability. Use private
vulnerability reporting on this repository. See [SECURITY.md](SECURITY.md).

## License

Dual-licensed [MIT](LICENSE-MIT) OR [Apache 2.0](LICENSE-APACHE). You may
choose either. The copyright notice in those files is ZeroClaw Labs' and
stays. The ZeroClaw name and logo are trademarks of ZeroClaw Labs.
