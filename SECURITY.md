# Security Policy

## Reporting a vulnerability

Do not open a public GitHub issue for a security vulnerability.

Report it through GitHub private vulnerability reporting on this repository:

https://github.com/tsmboa0/kinetic-vm/security/advisories/new

There is no separate security email yet. Please include:

- a description of the vulnerability
- steps to reproduce
- what an attacker gains
- a suggested fix, if you have one

## What the runtime enforces

Default autonomy is supervised.

- **ReadOnly** — the agent can read. It cannot write or run a shell.
- **Supervised** — the agent acts inside allowlists. This is the default.
- **Full** — the agent has full access inside the workspace sandbox.

The layers in front of a tool call:

1. Workspace isolation. File operations stay inside the workspace directory.
2. Path traversal blocking. `..` sequences and absolute paths are rejected.
3. Command allowlisting. Only approved commands can execute.
4. Forbidden paths. Critical system paths (`/etc`, `/root`, `~/.ssh`) stay blocked.
5. Rate and cost limits. Actions per hour and cost per day are capped.

Device signing keys stay behind the signer interface. A device key must never
hold operator rights over its own identity. Spending, once the vault exists,
goes through the vault limits only.

## Tests

```bash
cargo test -- security
cargo test -- tools::shell
cargo test -- tools::file_read
cargo test -- tools::file_write
```
