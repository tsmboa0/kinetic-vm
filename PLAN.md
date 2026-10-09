# KineticVM: Remaining Work Plan

Status as of Oct 3, 2026. Hackathon: Monad Metropolis, Trust, Identity & AI
Infrastructure track. Submissions close **Oct 13**.

Sections 1–9 are the October 3 plan and are partly stale (old vault
addresses, owner-only gas). Section 10 is in. Section 11.1 through 11.3
are in. The next implementation is **section 11.4**.

KineticVM gives physical AI devices a Monad identity (ERC-8004), an
owner-controlled wallet with spending limits, and signed onchain attestations
of what their actuators do. The runtime is a fork of ZeroClaw (MIT OR
Apache-2.0); the attribution in `NOTICE`, `LICENSE-MIT` and `LICENSE-APACHE`
must stay.

---

## 0. Toolchain and build notes (read first)

- The workspace needs **Rust 1.96**. `rust-toolchain.toml` pins `1.96.0`
  (with rustfmt and clippy). The machine default is still 1.93.1, so a bare
  `cargo` outside this repo is the wrong compiler.
- Always build with `CARGO_INCREMENTAL=0` to save disk. `target/` grows to
  ~40 GB and fills the disk. Free space by deleting old test binaries:
  `find target/debug/deps -type f -perm -u+x ! -name "*.*" -delete`
  (or `cargo clean`).
- Validation commands:
  ```bash
  CARGO_INCREMENTAL=0 cargo +1.96.0 fmt --all -- --check
  CARGO_INCREMENTAL=0 cargo +1.96.0 clippy --workspace --all-targets --features ci-all -- -D warnings
  CARGO_INCREMENTAL=0 cargo +1.96.0 test --workspace --features ci-all --no-fail-fast
  CARGO_INCREMENTAL=0 cargo +1.96.0 check --no-default-features --features agent-runtime,channel-telegram,hardware,peripheral-rpi
  ```
- A full clippy run takes about 1–3 minutes. The full test build and run takes
  much longer, because the workspace is huge (`schema.rs` alone is ~49k lines).
  While iterating, test individual crates with `-p <crate>`.
- The backup of the original ZeroClaw git history is at
  `~/zeroclaw-git-backup` (upstream SHA `afc47d333c90211f92ca3eca80cd2293c72c5025`).
- Remote: `origin = https://github.com/tsmboa0/kinetic-vm.git`.
  **Not pushed yet.** Decide whether to squash before the first push (step 6).

---

## 1. Done (committed)

| Commit | What |
|---|---|
| `496a8b1` | Import ZeroClaw `afc47d3` as the base |
| `8af00cf` | Remove apps (tauri, zerocode, zerorelay), xtask, tools, relay tests |
| `f4bc412` | Remove `web/` and `docs/` |
| `74ceda4` | Remove packaging (dist, k8s, nix, Docker, install.sh, setup.bat, release-plz, …) |
| `c576178` | Remove .github, .claude, CONTRIBUTING, fuzz, benches, …; add AGENTS.md, CLAUDE.md, pre-push hook |
| `260da75` | Remove `crates/zeroclaw-dist` |
| `1fbdb78` | Channels cut down to Telegram, MQTT, webhook, filesystem, ACP (plus voice-wake) |
| `2cd0c04` | Gate the gateway port-recovery helpers on the `gateway` feature |
| `445aafa` | Providers cut down to openai, anthropic, gemini, openrouter, ollama, hailo_ollama, custom |

Kept on purpose: all firmware and boards, translations (Fluent), plugins,
memory, the gateway (the device's HTTP API), eval, SOPs, voice-wake, ACP,
`dev/config.template.toml` and `dev/config.harness-test.toml` (compiled in via
`include_str!`), `evals/`, `scripts/deploy-rpi.sh`, `rpi-config.toml`,
`zeroclaw.service`, `99-act-led.rules`, `.githooks/pre-commit`.

---

## 2. Provider trim: done (`445aafa`)

Kept: `openai`, `anthropic`, `gemini`, `openrouter`, `ollama`, `hailo_ollama`
and `custom` (any OpenAI-compatible URL, which still reaches Groq, DeepSeek,
LM Studio, vLLM and the rest). Also removed: the generic `/models`
context-window probe (only OpenRouter's own catalog reader is left) and the
per-provider runtime-preset recommendation (no kept family qualified).

Validation at commit time: fmt, clippy (`--all-targets -D warnings`), the lean
device check, and per-crate tests all pass, apart from these known failures:

- `openai::tests::responses_model_listing_carries_header_only_profile_and_surfaces_auth_failure`
  and `responses_provider_honors_runtime_proxy_config`: also fail on the commit
  before the trim.
- `hailo_ollama::connection_establishment_failure_does_not_quarantine_endpoint`
  and `web_search_tool::tests::test_serply_connection_refused_error_is_query_free`:
  macOS lets a connection to a bound-but-not-listening port time out instead of
  refusing it. Should pass on Linux CI.
- `kinetic-buildinfo` `build_id_resolves_inside_a_real_checkout`: fails only
  while the working tree is dirty.

Optional leftovers (not blocking):

- `config/src/schema/v2.rs` still maps legacy V2 names (`grok`, `qwen-intl`,
  `llama.cpp`, …) to dropped families. This only affects migrating old ZeroClaw
  configs. KineticVM has no legacy users, so the V1/V2 migration could be
  deleted wholesale later.
- `QuickstartTypeOption::default_runtime_profile` is now always `None`; drop
  the field together with its RPC mirror when touching quickstart next.
- Pricing tables (`pricing.rs`), `models_dev.rs` keys and Fluent keys may still
  name dropped providers.
- `opencode_session.rs` is still threaded through the compatible/OpenAI clients.

---

## 3. Remaining trimming (step 3 continued)

Tools trim is done. Kept: memory, http_request, web_fetch, web_search,
calculator, ask_user, escalate, image_info, poll, knowledge, llm_task,
pipeline, sessions, model_switch, SOP tools, cron/schedule, delegate,
file tools, shell, A2A, and all hardware/peripheral tools. A non-empty
tool allowlist no longer auto-admits names containing `__`.

| Commit | What |
|---|---|
| `73c3b5e` | pushover, weather, image_gen, git_forge, git_operations, discord_search |
| `1c505b5` | SaaS integrations (Composio, Jira, Notion, LinkedIn, Google Workspace, Microsoft 365, cloud_ops) plus project_intel, report templates, security_ops |
| `4ec7254` | coding-agent CLIs |
| `1859158` | browser and computer-use |
| `d540d5c` | email tools and email OAuth2 login |
| `ad4336c` | live canvas tool and gateway routes |
| `4c49537` | verifiable intent |
| `562493b` | leftover names after the trim |
| `a802da1` | MCP client, config (`McpConfig` / `McpServerConfig`), and `Role::Mcp` |
| `2025f5f` | `zeroclaw update` and the gateway routes that shelled out to it. Restart classification on `/api/status` stays. `check_updates` / `allow_self_upgrade` stay on the status payload for a later updater. |
| `c6489e3` | Registries: plugins → `tsmboa0/kinetic-plugins`, skills → `tsmboa0/kinetic-skills`, locales and desktop releases → `tsmboa0/kinetic-vm` (`main`, not `master`). Those sibling repos do not exist yet; fetches fail closed until they do, or until this repo is pushed. |

Each remaining item gets its own commit, with clippy and tests run after each.

1. **Deferred cleanups** (optional, only if time allows):
   - Config structs for deleted channels still in `schema.rs` (Discord,
     Slack, Matrix, WhatsApp, …), plus their re-exports and test
     constructors in `src/config/mod.rs`.
   - The empty QR pairing hook (`QrPairingChannel {}`) in the
     channels/gateway.
   - Relay removal (woven into config, enroll and daemon).
   - Orphan comments in the root `Cargo.toml` (around lines 186–191).
   - `opencode_session.rs`.

---

## 4. Rename ZeroClaw → KineticVM: done (`cdbbd53`)

Done in one commit after the trim. Crates are `kinetic-*`, the binary is
`kinetic`, env vars are `KINETIC_*`, paths are `~/.kinetic`, and
`rust-toolchain.toml` pins 1.96.0. `ProxyScope::Zeroclaw` is
`ProxyScope::Kinetic` so `scope = "kinetic"` still deserializes.

Left as ZeroClaw on purpose, for the identity-files step:

- `LICENSE-MIT`, `LICENSE-APACHE`, and `NOTICE` (the ZeroClaw Labs credit).
- Root `README.md` (including the trademark sentence). Homepage and docs
  URLs in `Cargo.toml` still point at `zeroclaw.com` / `docs.zeroclaw.com`.
- Root `AGENTS.md` still says this is a modified fork of ZeroClaw.
- Upstream citations stay `github.com/zeroclaw-labs/zeroclaw`. The package
  `repository` field and the OpenRouter `HTTP-Referer` point at
  `https://github.com/tsmboa0/kinetic-vm`.

Validation at commit time: `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets -- -D warnings`, the lean device check
(`agent-runtime,channel-telegram,hardware,peripheral-rpi`), and workspace
tests with `--exclude kinetic-buildinfo` (this file is untracked, so the
build-id test would see a dirty tree). The only failures were the known
macOS flakes: Hailo and Serply classify a connection to a bound-but-not-
listening port as a timeout.

## 5. Identity files: done (`7b5bcd6`)

- `README.md` is the KineticVM pitch, build, quick start, and layout. It
  credits ZeroClaw and says the registry, vault, and attestor are not in
  this tree yet.
- `NOTICE` leads with "KineticVM includes code from ZeroClaw © ZeroClaw
  Labs, MIT OR Apache-2.0" and keeps the upstream copyright, official
  repository, and contributor attribution. `LICENSE-MIT` and
  `LICENSE-APACHE` were not edited.
- `SECURITY.md` points reports at GitHub private vulnerability reporting
  on `tsmboa0/kinetic-vm`. There is no security email yet. The removed
  Docker and docs-book instructions are gone.
- `AGENTS.md` names `crates/kinetic-runtime/src/i18n.rs`. `CLAUDE.md`
  still points at `AGENTS.md`.
- `Cargo.toml` homepage and documentation are the GitHub repo. There is
  no KineticVM website.

Comments and a few CLI strings still mention `docs/book/...`. That book
was removed earlier. Cleaning those pointers is separate from this step.

## 6. First push: history is on `origin/main`

Kept the import commit plus the later commits. Not squashed.

CI is `.github/workflows/ci.yml`. It runs on `main`, pull requests, and
manual dispatch:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`, then
  `cargo test --workspace --locked --no-fail-fast`
- the lean device check
  (`agent-runtime,channel-telegram,hardware,peripheral-rpi`)

The lean job has its own cache key so it does not reuse the default-feature
build. Push the workflow, then confirm the Actions run is green. It has not
been observed on GitHub yet.

---

## 7. Monad layer (the part judges score; most time goes here)

### 7.1 Contracts (`contracts/`, Foundry): deployed on Monad testnet

`forge test` — 30 passed. Broadcast on chain 10143 from
`0x64772107fC23f7370C90EA0aBd29ee6117B97f77`. `contracts/script/Smoke.s.sol`
claimed agent **2005**, deposited, allowlisted, topped up gas, paid, and
attested. `contracts/deployments/10143.json` has the live addresses:

- KineticRegistry `0xBf2E634F8DA4C8C02979C1A2CcAD113eFb259132`
- DeviceVault `0xD01eEa46c7E054f98dde0ed58DB5Da73ED4D22fD`
- KineticAttestor `0xAb523187C7687743B29daf3468891E339CbF8f82`

Deploy txs: registry `0xd1fa7747…15f03b7`, vault `0x2b2a0679…b0343881`,
setVault `0x07139242…7f6ebb22`, attestor `0xf2604bc7…4548d1e6`.
Smoke: claim `0x49903205…68883933`, attest `0x646f8363…44e35c17`.
Validation request hash `0xa1ac0a2e…d4a32ed9`, response 100, tag
`kinetic-action`.

Testnet Validation is `0x8004Cb1BF31DAf7788923b405b754f57acEB4272` and
Reputation is `0x8004B663056A597Dffe9eCcC1965A193B7388713` (the CREATE2
addresses used on every ERC-8004 testnet, including chain 10143).

`claim` mints on the Identity Registry, then transfers the NFT to the owner
because `register` always mints to the caller. The device signs an EIP-712
claim ticket (device, owner, nonce, deadline; at most one day) and is never
the owner or an operator. The owner recorded at bind is snapshotted. If
`ownerOf` changes, spending and attestation revert until anyone calls
`releaseTransferred`, which clears the device and pauses the vault. The new
owner can `link` a new device; the vault stays paused and the allowlist is
cleared. After `revoke`, the same agent can be linked to a new device the
same way. The owner approves `KineticAttestor` before an attestation.
Loosening a vault limit needs the owner's transaction or their EIP-712
signature.

What is still outside this code: an external audit. The contracts are not
upgradeable and have no global pause. A response of 100 means the device key
signed the record. It does not prove the actuator moved.

- `KineticRegistry`:
  - Maps a device key to its ERC-8004 agent ID.
  - `claim(device, signature, deadline, agentURI, limits)`: the owner
    signs one transaction; the device's EIP-712 claim ticket proves it holds
    the key and names this owner.
  - Mints or links the identity in the ERC-8004 Identity Registry.
  - `deviceToAgent(addr)` (read at boot); `activeDevice` reverts if the NFT
    owner has changed. The live owner is `ownerOf`.
  - Revoke and rotate the device key (owner only). `releaseTransferred`
    after an NFT transfer (anyone).
- `DeviceVault`:
  - Per-transaction cap, rolling 24-hour cap, recipient allowlist, pause,
    owner withdraw.
  - Gas top-up is owner-only, sent to the active device, and still counts
    against the caps.
  - The owner can tighten limits from Telegram; **loosening needs an owner
    wallet signature**.
  - A rebind replaces the caps, stays paused, and clears the allowlist and
    the spend window.
  - Spending only ever goes through these limits (AGENTS.md §2).
- `KineticAttestor` (acts as the ERC-8004 validator):
  - The device signs an EIP-712 `ActionProof` (agentId, action, params hash,
    request URI, timestamp, nonce). The URI is inside the signature.
  - The attestor verifies the signature against `activeDevice`.
  - It then calls `validationRequest` and `validationResponse` on the
    Validation Registry.
  - The owner approves the **attestor contract** as ERC-721 operator,
    **never the device key**.
- ERC-8004 addresses:
  - Monad mainnet (chain 143):
    - Identity `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432`
    - Reputation `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63`
    - Validation `0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58`
  - Testnet (10143): Identity `0x8004A818BFB912233c491871b3d84c89A494BD9e`.
    Confirm the testnet Validation/Reputation addresses before deploying.
- ERC-8004 rules to respect:
  - `validationRequest` may only be called by the owner or an operator.
  - `validationResponse` may only be called by the named validator.
  - `giveFeedback` is forbidden for the owner and operators.
  - `setAgentWallet` needs an EIP-712 signature.
- Foundry tests for each contract, then a deploy script to Monad testnet.
  Record the addresses in `contracts/deployments/10143.json`.

### 7.2 `crates/kinetic-chain` (Rust, alloy): software key and boot read are in

- `DeviceSigner` plus a software key. First boot generates it, encrypts it
  with the install secret store, and never logs it. The SE050 backend is
  still later (same trait; low-s and recovery id).
- `boot` reads `deviceToAgent`. Unclaimed returns the address text a claim
  screen can put in a QR. The claim ticket is signed only once the owner
  address is known (`sign_claim` asks the registry for `hashClaim`).
  Claimed reads `activeDevice`, `ownerOf`, and the vault limits. No local
  cache.
- Config section `[chain]` (off by default) holds chain id 10143, the public
  testnet RPC, and the three deployed addresses. Later fields (network,
  claim page, owner, caps) are in 7.5.
- `pay` and `attest` submit from the device key. The attestor still hashes
  the proof; the client only signs that digest.

### 7.3 Agent tools: in

`monad_identity`, `monad_attest`, `vault_status`, and `vault_pay` register
when `[chain]` is enabled. `vault_pay` calls `DeviceVault.pay` from the
device key, so the caps and allowlist still apply. A successful
`gpio_write`, `gpio_rpi_write`, `gpio_rpi_blink`, or `set_device` is then
signed and submitted to the attestor. The user-facing lines are Fluent
keys in `locales/en/cli.ftl` and `locales/en/tools.ftl`.

Next: the owner setup and claim link (7.5).

### 7.4 Telegram owner commands: in

The bot, slash menu, sender id, and reply path were already in the runtime.
These commands sit on that path and only run when the channel is `telegram`.

- `/link` asks the owner to personal-sign a text that includes the chat id,
  then stores `{config_dir}/owner.link` (mode `0600`). A signature for one
  chat cannot link another. Every later command re-reads `ownerOf` and
  rejects the chat if the NFT moved.
- `/status` shows identity, vault balance, caps, and the last few `Attested`
  logs. If the log read fails, the reply still includes the attestor explorer
  URL.
- `/pause` and a lower `/limits` do not broadcast. The device key cannot send
  those calls, so the reply tells the owner wallet to call `pause` or
  `tightenCaps` on the vault.
- `/resume`, a higher `/limits`, and `/approve <address>` return the contract
  EIP-712 digest. The owner pastes the signature and the device key relays
  `unpause`, `loosenCaps`, or `allowRecipient`. A personal signature is
  rejected by the vault. `/approve` does not raise a cap and does not invent
  a pending payment.

### 7.5 Shared contracts, claim link, and the setup the owner sees

The three deployed contracts are one shared installation. Many devices use
the same KineticRegistry, DeviceVault, and KineticAttestor. They are not a
factory: claiming a device does not deploy a new contract. The defaults in
`[chain]` are the Monad testnet deployment. Someone redeploys and repoints
`chain.registry`, `chain.vault`, and `chain.attestor` only when they need a
private copy.

`setVault` is once per registry deployment, and only the deployer who
created that registry can call it. It is already done on the published
testnet registry. A new device does not call it. A new private registry
does, once, after that deploy. `claim` reverts `VaultUnset` until then, and
a second call reverts `VaultAlreadySet`.

`/bind` and `/link` stay, and they are different proofs. `/bind` only shows
that a Telegram chat saw the pairing code. `/link` shows that this chat is
the NFT owner: the owner personal-signs a text that includes the chat id,
and the device checks it against `ownerOf`. The chain already rejects an
unauthorized contract call. Without `/link`, any paired chat could still
read the vault and start owner commands. Setup stores an owner address. It
does not know which Telegram user that wallet is. Order: `/bind`, then
claim, then `/link`.

The claim page is a public HTTPS URL. A phone wallet cannot open
localhost. `[chain].claim_url` defaults to `https://claim.kineticvm.xyz`
(the page we will host; anyone can use it). A project that hosts its own
page sets `chain.claim_url` to that origin. The device sends the link. The
page never sees the device key.

`kinetic claim-link` signs the claim ticket after `chain.owner` is set and
prints that URL. The query carries the device, owner, signature, deadline,
caps, chain id, device name, the three contract addresses, and the RPC, so
the shared page can submit against this device's deployment without calling
back to localhost. The signature is valid for 23 hours, inside the
contract's one-day maximum. The printed QR stays `kinetic:<address>`. Do
not put the signature in a QR.

The page sends two calls in one wallet confirmation: `claim(...)` with the
device signature, then `setApprovalForAll` for the attestor. The registry
cannot approve the attestor for the human. It only holds the NFT for a
moment inside `claim`. Deposit and `topUpGas` stay separate. They move MON,
and the amounts are not known at claim time.

`chain.network` is `testnet` by default. Mainnet (chain id 143) is refused
while any of the three addresses is still the published testnet contract,
or while the chain id is not 143. KineticVM contracts are not on mainnet.
Switching network later, in quickstart, changes the chain id, RPC, and the
three addresses together. Setting only the chain id against the testnet
addresses is refused.

The purple-and-white `KINETIC VM` mark is in. It prints once at the start
of `kinetic quickstart` and once at the start of an interactive `kinetic
start`, then the prompts or the logs. The loading bar runs only while
startup is waiting for the gateway and socket. A service journal and
`NO_COLOR` stay plain, and verbose mode skips the bar so log lines are
left intact.

Still to build:

- Tailor `kinetic quickstart` to device name, owner wallet, Telegram bot
  token, and the starting per-transaction and daily caps. Keep the model
  provider and API key. Drop the long ZeroClaw checklist. The device
  address is generated, not typed. The caps feed the claim and can be
  changed later from Telegram.
- `kinetic config get` with no path prints a short card: device address
  (from `device.key`, and only if that file already exists), claimed or
  not, owner, caps, chain, and whether Telegram is on. Reading config must
  not create the key.
- `kinetic start` is the runtime command. `kinetic daemon` stays a hidden
  alias so `kinetic service install` and existing scripts keep working.
  Service install still registers systemd or launchd. The process starts
  when the OS boots. It does not run before the OS.
- The installer is `install.sh`. One command downloads the release
  binary for Linux x86_64, Linux aarch64 (the Pi), and macOS (Apple
  Silicon and Intel) into `~/.local/bin`, and checks it against
  `SHA256SUMS`. `.github/workflows/release.yml` builds those binaries
  with the device feature set when a `v*` tag is pushed. Until the first
  tag is published, the command reports that the release is not there yet.

### 7.6 Claim page (`web/claim`)

Small page at the default claim URL. It reads the claim-link query, asks
the owner wallet to confirm `claim` and `setApprovalForAll` together, then
shows the agent ID and an explorer link. Deposit and gas top-up are
separate steps on the same page.

### 7.7 Docs (`docs/`, Mintlify)

Overview, quick start on Raspberry Pi with ESP32, architecture (identity,
vault, attestations), contracts reference, signer and SE050, Telegram
commands, building your own device.

### 7.8 Demo: the vending machine

- The ESP32 drives the motors over serial and connects to a Pi.
- Flow:
  1. Claim the machine via QR.
  2. A customer pays.
  3. The vault receives the payment.
  4. The agent vends (actuator).
  5. An attestation lands onchain.
  6. The owner sees `/status` in Telegram.
- Record a 2–3 minute video and write the submission text.
- Pitch line: "a tokenized machine: owning the NFT means owning the
  machine". Don't lead with "tokenization".
- Roadmap items to mention, not build: gas relayer, data marketplace
  (aggregate machine data with provenance).

---

## 8. Suggested schedule

| Day | Work |
|---|---|
| Oct 3–4 | Tools/MCP, `update`, registries, the rename (`cdbbd53`), identity files (`7b5bcd6`), and the first push are done. CI workflow is local until the next push. Next: contracts (7.1) |
| Oct 4–7 | Agent tools and Telegram owner commands are in (7.3, 7.4). Shared-contract setup and `kinetic claim-link` are in progress (7.5) |
| Oct 7–9 | Quickstart, config card, `kinetic start`, banner (7.5) |
| Oct 9–10 | Claim page (7.6) |
| Oct 11 | End-to-end vending machine demo on hardware (7.8) |
| Oct 12 | Docs, README, video, submission (7.7) |
| Oct 13 | Buffer, then submit |

To save time if it runs short: skip the deferred cleanups, the SE050 backend
(use the software key; present SE050 as supported by design) and the Mintlify
polish. Never skip the contracts, attestations or the demo.

---

## 9. Helper scripts (in `/tmp`, lost on reboot)

- `/tmp/rm_tests.py`: reads `file:line` from stdin and deletes the enclosing
  `#[test]` fn. Feed it the error lines from
  `clippy --message-format short`.
- `/tmp/rm_items.py FILE REGEX...`: deletes brace-delimited items whose
  header matches.
- `/tmp/rust_scan.py`: a string- and raw-string-aware brace matcher used by
  both scripts. An earlier version mishandled multi-line raw strings; this
  one is fixed.
- `/tmp/strip_cfg.py`, `/tmp/fix_orphans.py`, `/tmp/rmdeps.py`: from the
  channel cut.
- `/tmp/drop_families.txt`: the 72 dropped provider families.

If they're gone, recreating them is quick; the approach is to remove the
config slot, let the compiler list every reference, and delete or port each
one.

---

## 10. Builder path and operation records (Oct 8)

A third-party device builder does not deploy contracts. They install the
binary, run quickstart, claim the device, and fund the vault with
`/deposit`. Their own logic is a plugin package plus an SOP. The host
keeps the device key, the serial link, the vault, and attestation. The
package shape and the commands that produce it are section 11.

### 10.1 Defaults that are on

- `plugins.enabled` and `plugins.auto_discover` default to on. An empty
  `~/.kinetic/plugins` directory loads nothing. An operator turns either
  off by setting it false. `#[serde(default)]` on a bool stays false, so
  the fields use `default_plugins_enabled` / `default_plugins_auto_discover`.
- SOP runtime defaults on. `sops_dir` defaults to `~/.kinetic/sops`. An
  empty directory runs nothing. An explicit empty `sops_dir` stays off.
- Seed workspace prompts (`SOUL.md`, `IDENTITY.md`, and the rest) are still
  a personal-assistant persona. Replacing them is the builder's prompt and
  comes after this slice. The fixed preamble stays first and still wins on
  tool honesty and safety.

### 10.2 Customer Telegram does not raise approval cards

Supervised mode used to prompt for every tool outside a short allowlist,
and the card was delivered into the customer chat. That stalls an
unattended operation. A sale is one example. The same channel is how any
integrated system talks to a person.

On the Telegram back-channel (`for_non_interactive_backchannel`), every
tool is auto-approved except `shell`, `file_write`, `file_edit`, and
`vault_pay`. `always_ask` still prompts, and it is checked first. The CLI
supervised prompt is unchanged. `for_non_interactive` (not the back-channel)
still prompts for unknown tools.

Plugin tools are not wrapped in an attestation. An actuator attestation
is the existing one on a successful `gpio_write` (and the other actuators
already in that list). A system with no actuator still attests through
`monad_attest`.

### 10.3 Local records file

One file per install, written by the host, outside the agent workspace:

`~/.kinetic/records/operations.jsonl`

Resolved from `config.toml`'s directory via `Config::operation_records_path`,
so a `KINETIC_CONFIG_DIR` install lands beside that config. The model must
not treat this file as a note. Codes, chat ids, and wallets are excluded.

Each line is canonical JSON for one completed operation. The builder
defines the schema. A vend line, a soil reading, and a payment are all
the same kind of record. Its keccak256 is the `paramsHash` of the
`monad_attest` the device already signs for that operation. A buyer
rehashes the exact line and matches `responseHash` on the existing
ERC-8004 validation response. There is no second attestation that binds
an IPFS CID.

The append is wired through `monad_attest`. The builder lists fields under
`[records]`. When that list is empty, attestation is unchanged and no line
is written. When it is set, the tool checks the caller's JSON, signs the
canonical line, and appends that same line after the chain write succeeds.
A pin write stays an actuator proof. It does not become an operation record
unless the builder also calls `monad_attest` with their schema. The host
does not add chat ids, wallets, or access codes.

### 10.4 Listing, later in this repo

`/sell_data` is an owner Telegram command. A native tool reads the JSONL
and POSTs the file plus a manifest to the marketplace API. The device
does not hold a Pinata key and does not call `monad_attest` for the CID.

Quickstart asks for a marketplace API key and stores it as an optional
secret. Missing key means listing is unavailable. The device still runs.

### 10.5 Marketplace service, not this repo

Signup, the list API, the Pinata upload, and the buyer verify page are a
separate service. It returns the CID. Row-level verification stays the
hash match against existing action attestations.

### 10.6 Out of this slice

ESP32 analog `soil_read`, the peripheral SOP listener, an in-process Rust
trait beside the device key, wrapping plugin tools, and rewriting the seed
persona. Soil stays a cron poll of `gpio_read` until the firmware command
exists. A keypad emits MQTT; it is not a cron poll.

## 11. Builder package (Oct 9)

One directory is one integration. It holds several tools, one procedure,
an optional skill, and an example of the operation schema. State is not a
file in that directory. The tools read and write it through the host, and
every tool in the package shares that state and the package config.

```text
<name>/
  Cargo.toml
  manifest.toml            permissions, egress hosts, config schema
  src/lib.rs               the tools, plus a few lines that save one state key
  wit/v0/                  the host WIT this package binds
  sop/SOP.toml
  sop/SOP.md
  skills/<name>/SKILL.md   optional; delete the directory and drop `skill` if unused
  records.example.toml     the `[records]` block, with a comment
  .gitignore
```

`kinetic build` does not make the device heavier. It runs `cargo` that is
already installed on the developer machine, writes `plugin.wasm`, and
records `wasm_path` and `wasm_sha256` in the manifest. The Pi install does
not include rustc, cargo, or the `wasm32-wasip2` target. If either is
missing, the command says so and stops. The compiler message stays visible.

Do these in order. The scaffold comes after a package can hold more than
one tool, so the skeleton matches what the host loads.

### 11.1 Several tools from one plugin

In. `wit/v0/tool.wit` has a `toolbox` interface and a `tools-plugin` world:

- `list-tools` returns each tool's name, description, and parameter schema.
- `execute(name, args)` runs one of them.

The host registers each name as its own tool. An empty list, a blank or
illegal name, or a duplicate name refuses the package. Config and state
stay scoped to the package, so the tools share them.

A component that still `export tool` loads as it does today. The old world
stays. Existing fixtures are unchanged.

### 11.2 `kinetic plugin create <dir>`

In. The directory name is the package name: lowercase letters, digits, and
underscores, and it must not start with a digit. The command refuses a path
that already exists.

It writes the skeleton above with a short note at the top of each file and a
tiny generic starter in the body. Two tools (`ping` and `note`), one SOP
step, one record field, one short skill, and a state read/write in
`src/lib.rs`. `wit/v0` is copied from the host so the package binds the
toolbox world. The starter is something to replace, not a vending machine.

### 11.3 `kinetic build`

In. `kinetic build` in the package directory, or `kinetic build <dir>`,
runs the `cargo` already on that machine. It compiles for `wasm32-wasip2`,
copies the component to the manifest's `wasm_path` (the starter uses
`plugin.wasm`), and writes `wasm_sha256`. Cargo's own messages stay on the
terminal. A missing Cargo or a missing `wasm32-wasip2` target stops with
those words and does not start a compile. The package `Cargo.toml` is its
own workspace, so a parent workspace does not absorb it. The Kinetic binary
does not contain rustc.

### 11.4 `kinetic plugin install <dir>`

Install already copies the component into `~/.kinetic/plugins` and checks
that it loads. Extend it to copy `sop/` into the sops directory and print
`records.example.toml` so the operator can put it under `[records]`. The
live schema stays in the device config. The plugin does not become a second
copy of it.
