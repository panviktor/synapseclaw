<p align="center">
  <img src="docs/assets/synapseclaw.png" alt="SynapseClaw" width="200" />
</p>

<h1 align="center">SynapseClaw</h1>

<p align="center">
  <strong>Multi-agent Rust runtime with IPC broker, web dashboard, and hexagonal core.</strong><br>
  Models, tools, memory, channels, and execution — one deployable binary.
</p>

<p align="center">
  <a href="LICENSE-APACHE"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache%202.0-blue.svg" alt="License: MIT OR Apache-2.0" /></a>
</p>

<p align="center">
  <a href="#quick-start">Getting Started</a> |
  <a href="docs/README.md">Docs Hub</a> |
  <a href="docs/SUMMARY.md">Docs TOC</a> |
  <a href="docs/use/skills/quickstart.md">Skills Quickstart</a>
</p>

<p align="center">
  <strong>Quick Routes:</strong>
  <a href="docs/reference/README.md">Reference</a> ·
  <a href="docs/operate/README.md">Operations</a> ·
  <a href="docs/use/skills/troubleshooting.md">Skills Troubleshooting</a> ·
  <a href="docs/understand/architecture.md">Architecture</a> ·
  <a href="docs/extend/README.md">Extend</a>
</p>

---

## What is SynapseClaw

SynapseClaw is a **single-binary Rust runtime** for autonomous agent workflows. It runs one agent or a broker-managed agent family from the same binary — no containers, no orchestrators, no JVM.

The project started as a fork of [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) and has since diverged into an independent project with its own architecture, IPC system, web dashboard, and modular core.

### Key capabilities

- **Multi-agent IPC broker** — trust-aware inter-agent messaging with directional ACLs, quarantine, ephemeral agent spawning, and delivery controls.
- **Web operator dashboard** — fleet topology, agent workbench, chat sessions, activity feed, cron management — all from one frontend shell.
- **Hexagonal core** (`synapse_domain`) — pure business logic as a workspace crate with 0 upstream dependencies, 10 use cases, 180+ tests. Ports & adapters architecture.
- **Trait-driven pluggability** — providers, channels, tools, memory, observers, and runtime adapters are all swappable via traits.
- **Channel support** — Telegram, Discord, Slack, Matrix (E2EE), Mattermost, web chat, and more.
- **Security hardening** — Ed25519 identity, PromptGuard integration, execution profiles, tool allowlists, workspace scoping.
- **Lean runtime** — < 5 MB RAM, < 10 ms startup, ~9 MB binary. Runs on $10 ARM boards.

### Architecture overview

```
crates/domain/                   crates/adapters/                Infrastructure
synapse_domain (24K LOC)         composition root (55K LOC)      ├── gateway/ (HTTP + WS)
├── domain/                      ├── core/ (synapse_adapters)    ├── cron/scheduler
│   ├── channel, conversation    ├── channels/ (34K)             ├── security/
│   ├── ipc, memory, approval    ├── tools/ (37K)                ├── channels/ (transport)
│   ├── run, spawn, config       ├── security/ (10K)             └── tools/ (execution)
│   └── message                  ├── memory/ (8K)
├── ports/ (12 traits)           ├── providers/ (20K)
├── application/services/ (6)    ├── observability/ (5K)
└── application/use_cases/ (10)  ├── cron-store/ (3K)
                                 ├── mcp/ (3K)
                                 ├── infra/ (5K)
                                 └── onboard/ (7K)
```

**Dependency rules** — domain depends on nothing; adapters depend on domain; the binary composes everything.

For current architecture docs, start with [`docs/understand/architecture.md`](docs/understand/architecture.md).
Historical phase plans, audits, and fork-era notes are archived under [`docs/deprecated/`](docs/deprecated/README.md).

## Prerequisites

<details>
<summary><strong>Windows</strong></summary>

#### Required

1. **Visual Studio Build Tools** (provides the MSVC linker and Windows SDK):

    ```powershell
    winget install Microsoft.VisualStudio.2022.BuildTools
    ```

    During installation (or via the Visual Studio Installer), select the **"Desktop development with C++"** workload.

2. **Rust toolchain:**

    ```powershell
    winget install Rustlang.Rustup
    ```

    After installation, open a new terminal and run `rustup default stable` to ensure the stable toolchain is active.

3. **Verify** both are working:
    ```powershell
    rustc --version
    cargo --version
    ```

#### Optional

- **Docker Desktop** — required only if using the [Docker sandboxed runtime](#runtime-support-current) (`runtime.kind = "docker"`). Install via `winget install Docker.DockerDesktop`.

</details>

<details>
<summary><strong>Linux / macOS</strong></summary>

#### Required

1. **Build essentials:**
    - **Linux (Debian/Ubuntu):** `sudo apt install build-essential pkg-config`
    - **Linux (Fedora/RHEL):** `sudo dnf group install development-tools && sudo dnf install pkg-config`
    - **macOS:** Install Xcode Command Line Tools: `xcode-select --install`

2. **Rust toolchain:**

    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    ```

    See [rustup.rs](https://rustup.rs) for details.

3. **Verify** both are working:
    ```bash
    rustc --version
    cargo --version
    ```

#### One-Line Installer

```bash
curl -LsSf https://raw.githubusercontent.com/panviktor/synapseclaw/master/install.sh | bash
```

#### Compilation resource requirements

| Resource       | Minimum | Recommended |
| -------------- | ------- | ----------- |
| **RAM + swap** | 2 GB    | 4 GB+       |
| **Free disk**  | 6 GB    | 10 GB+      |

If your host is below the minimum, use pre-built binaries:

```bash
./install.sh --prefer-prebuilt
```

#### Optional

- **Docker** — required only if using the [Docker sandboxed runtime](#runtime-support-current) (`runtime.kind = "docker"`). Install via your package manager or [docker.com](https://docs.docker.com/engine/install/).

> **Note:** The default `cargo build --release` uses `codegen-units=1` to lower peak compile pressure. For faster builds on powerful machines, use `cargo build --profile release-fast`.

</details>

## Quick Start

### Homebrew (macOS/Linuxbrew)

```bash
brew install synapseclaw
```

### One-click bootstrap

```bash
git clone https://github.com/panviktor/synapseclaw.git
cd synapseclaw
./install.sh

# Optional: bootstrap dependencies + Rust on fresh machines
./install.sh --install-system-deps --install-rust

# Optional: pre-built binary first (recommended on low-RAM/low-disk hosts)
./install.sh --prefer-prebuilt
```

### Pre-built binaries

Release assets are published for Linux (x86_64, aarch64, armv7), macOS (x86_64, aarch64), and Windows (x86_64).

Download: <https://github.com/panviktor/synapseclaw/releases/latest>

### Build from source

```bash
git clone https://github.com/panviktor/synapseclaw.git
cd synapseclaw
cargo build --release --locked
cargo install --path . --force --locked

export PATH="$HOME/.cargo/bin:$PATH"
```

### Usage

```bash
# Quick setup
synapseclaw onboard --api-key sk-... --provider openrouter

# Or preset-first guided wizard
synapseclaw onboard

# Chat
synapseclaw agent -m "Hello, SynapseClaw!"

# Interactive mode
synapseclaw agent

# Start the gateway (webhook server)
synapseclaw gateway

# Start full autonomous runtime
synapseclaw daemon

# Check status
synapseclaw status

# Run system diagnostics
synapseclaw doctor

# Manage background service
synapseclaw service install
synapseclaw service status
```

`synapseclaw onboard` starts with simple provider presets (`ChatGPT / Codex`,
`Claude`, `OpenRouter`, `Local`, `Advanced`) and expands those into lane-aware
routing under the hood.

## Models

### Quick: pick a model

```bash
# Set the default model directly
synapseclaw models set claude-sonnet-4-6

# Check what's active
synapseclaw models status

# List cached models for your provider
synapseclaw models list

# Pull the latest models from the provider API
synapseclaw models refresh
```

### Editable model catalog (advanced)

SynapseClaw ships with a **built-in catalog** (`model_catalog.json`) containing presets,
30+ providers, curated model lists, pricing, context-window profiles, embedding profiles,
and route aliases (`cheap`, `qwen36`, `gemma31b`, …).

To override anything locally:

```bash
synapseclaw models catalog init          # writes model_catalog.json next to config.toml
synapseclaw models catalog init --force  # overwrite if it already exists
```

The local file is **merged over** the built-in catalog on startup:
- matching entries (by `id`, `provider`, `model`, or `hint`) **replace** built-in values
- new entries are **added** — custom providers, presets, aliases
- missing sections are ignored — include only what you want to change

**Locations** (checked in order):

| Path | Scope |
|------|-------|
| `~/.synapseclaw/model_catalog.json` | Global (all agents) |
| `~/.synapseclaw/agents/<name>/model_catalog.json` | Per-agent override |
| `<--config-dir>/model_catalog.json` | Custom config directory |

**Full command reference:**

| Command | What it does |
|---------|-------------|
| `synapseclaw models catalog init` | Create editable catalog from built-in seed |
| `synapseclaw models catalog init --force` | Overwrite existing catalog |
| `synapseclaw models catalog status` | Show path and active/not active |
| `synapseclaw models catalog path` | Print catalog file path |
| `synapseclaw models refresh` | Pull latest models from provider APIs |
| `synapseclaw models list` | List cached provider models |
| `synapseclaw models set <model>` | Set default model in config |
| `synapseclaw models status` | Show current model configuration |

> **Dev fallback (no global install):** prefix commands with `cargo run --release --` (example: `cargo run --release -- status`).

## Subscription Auth (OpenAI Codex / Claude Code)

SynapseClaw supports subscription-native auth profiles (multi-account, encrypted at rest).

```bash
# OpenAI Codex OAuth
synapseclaw auth login --provider openai-codex --device-code

# Claude Code / Anthropic
synapseclaw auth paste-token --provider anthropic --profile default --auth-kind authorization

# Run with subscription auth
synapseclaw agent --provider openai-codex -m "hello"
synapseclaw agent --provider anthropic -m "hello"
```

## Documentation

- Documentation hub: [`docs/README.md`](docs/README.md)
- Docs TOC: [`docs/SUMMARY.md`](docs/SUMMARY.md)
- Start guide: [`docs/start/what-is-synapseclaw.md`](docs/start/what-is-synapseclaw.md)
- User guide: [`docs/use/README.md`](docs/use/README.md)
- Skills quickstart: [`docs/use/skills/quickstart.md`](docs/use/skills/quickstart.md)
- Skills lifecycle: [`docs/reference/skill-lifecycle.md`](docs/reference/skill-lifecycle.md)
- Skills API: [`docs/reference/skills-api.md`](docs/reference/skills-api.md)
- Operations: [`docs/operate/README.md`](docs/operate/README.md)
- Architecture: [`docs/understand/architecture.md`](docs/understand/architecture.md)
- Developer guide: [`docs/extend/README.md`](docs/extend/README.md)
- Archived historical docs: [`docs/deprecated/README.md`](docs/deprecated/README.md)
- Security: [`SECURITY.md`](SECURITY.md)

## License

SynapseClaw is dual-licensed:

| License | Use case |
|---|---|
| [MIT](LICENSE-MIT) | Open-source, research, academic, personal use |
| [Apache 2.0](LICENSE-APACHE) | Patent protection, institutional, commercial deployment |

You may choose either license. See [CLA.md](docs/deprecated/contributing/cla.md) for the archived contributor agreement.

### Trademark

The **SynapseClaw** name and logo are trademarks of SynapseClaw Labs. See [TRADEMARK.md](docs/deprecated/maintainers/trademark.md) for permitted and prohibited uses.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Implement a trait, submit a PR:

- New `Provider` → `crates/adapters/providers/src/`
- New `Channel` → `crates/adapters/channels/src/`
- New `Tool` → `crates/adapters/tools/src/`
- New `Memory` → `crates/adapters/memory/src/`
- New `Observer` → `crates/adapters/observability/src/`

---

**SynapseClaw** — Multi-agent runtime. Single binary. Deploy anywhere.
