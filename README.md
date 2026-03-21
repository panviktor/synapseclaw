<p align="center">
  <img src="docs/assets/synapseclaw.png" alt="SynapseClaw" width="200" />
</p>

<h1 align="center">SynapseClaw 🦀</h1>

<p align="center">
  <strong>Single-binary Rust runtime for single-agent and brokered multi-agent workflows.</strong><br>
  Models, tools, memory, channels, and execution behind one deployable runtime.
</p>

<p align="center">
  <a href="LICENSE-APACHE"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache%202.0-blue.svg" alt="License: MIT OR Apache-2.0" /></a>
  <a href="https://github.com/panviktor/synapseclaw/graphs/contributors"><img src="https://img.shields.io/github/contributors/panviktor/synapseclaw?color=green" alt="Contributors" /></a>
  <a href="https://buymeacoffee.com/argenistherose"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Donate-yellow.svg?style=flat&logo=buy-me-a-coffee" alt="Buy Me a Coffee" /></a>
  <a href="https://x.com/synapseclaw?s=21"><img src="https://img.shields.io/badge/X-%40synapseclaw-000000?style=flat&logo=x&logoColor=white" alt="X: @synapseclaw" /></a>
  <a href="https://www.facebook.com/groups/synapseclaw"><img src="https://img.shields.io/badge/Facebook-Group-1877F2?style=flat&logo=facebook&logoColor=white" alt="Facebook Group" /></a>
  <a href="https://www.reddit.com/r/synapseclaw/"><img src="https://img.shields.io/badge/Reddit-r%2Fsynapseclaw-FF4500?style=flat&logo=reddit&logoColor=white" alt="Reddit: r/synapseclaw" /></a>
</p>
<p align="center">
  🌐 <strong>Languages:</strong>
  <a href="README.md">🇺🇸 English</a> ·
  <a href="README.zh-CN.md">🇨🇳 简体中文</a> ·
  <a href="README.ja.md">🇯🇵 日本語</a> ·
  <a href="README.ko.md">🇰🇷 한국어</a> ·
  <a href="README.vi.md">🇻🇳 Tiếng Việt</a> ·
  <a href="README.tl.md">🇵🇭 Tagalog</a> ·
  <a href="README.es.md">🇪🇸 Español</a> ·
  <a href="README.pt.md">🇧🇷 Português</a> ·
  <a href="README.it.md">🇮🇹 Italiano</a> ·
  <a href="README.de.md">🇩🇪 Deutsch</a> ·
  <a href="README.fr.md">🇫🇷 Français</a> ·
  <a href="README.ar.md">🇸🇦 العربية</a> ·
  <a href="README.hi.md">🇮🇳 हिन्दी</a> ·
  <a href="README.ru.md">🇷🇺 Русский</a> ·
  <a href="README.bn.md">🇧🇩 বাংলা</a> ·
  <a href="README.he.md">🇮🇱 עברית</a> ·
  <a href="README.pl.md">🇵🇱 Polski</a> ·
  <a href="README.cs.md">🇨🇿 Čeština</a> ·
  <a href="README.nl.md">🇳🇱 Nederlands</a> ·
  <a href="README.tr.md">🇹🇷 Türkçe</a> ·
  <a href="README.uk.md">🇺🇦 Українська</a> ·
  <a href="README.id.md">🇮🇩 Bahasa Indonesia</a> ·
  <a href="README.th.md">🇹🇭 ไทย</a> ·
  <a href="README.ur.md">🇵🇰 اردو</a> ·
  <a href="README.ro.md">🇷🇴 Română</a> ·
  <a href="README.sv.md">🇸🇪 Svenska</a> ·
  <a href="README.el.md">🇬🇷 Ελληνικά</a> ·
  <a href="README.hu.md">🇭🇺 Magyar</a> ·
  <a href="README.fi.md">🇫🇮 Suomi</a> ·
  <a href="README.da.md">🇩🇰 Dansk</a> ·
  <a href="README.nb.md">🇳🇴 Norsk</a>
</p>

<p align="center">
  <a href="#quick-start">Getting Started</a> |
  <a href="https://raw.githubusercontent.com/panviktor/synapseclaw/master/install.sh">One-Click Setup</a> |
  <a href="docs/README.md">Docs Hub</a> |
  <a href="docs/SUMMARY.md">Docs TOC</a>
</p>

<p align="center">
  <strong>Quick Routes:</strong>
  <a href="docs/reference/README.md">Reference</a> ·
  <a href="docs/ops/README.md">Operations</a> ·
  <a href="docs/ops/troubleshooting.md">Troubleshoot</a> ·
  <a href="docs/security/README.md">Security</a> ·
  <a href="docs/hardware/README.md">Hardware</a> ·
  <a href="docs/contributing/README.md">Contribute</a>
</p>

<p align="center">
  <strong>Run one agent or a broker-managed agent family from the same runtime.</strong>
</p>

<p align="center">
  SynapseClaw is the <strong>runtime operating system</strong> for agentic workflows — infrastructure that abstracts models, tools, memory, and execution so agents can be built once and run anywhere.
</p>

<p align="center">
  It starts as a lean single-agent runtime and grows into a broker-managed multi-agent control plane without changing the core deployment model.
</p>

<p align="center"><code>Trait-driven architecture · secure-by-default runtime · provider/channel/tool swappable · pluggable everything</code></p>

<!-- BEGIN:WHATS_NEW -->
<!-- END:WHATS_NEW -->

### Why teams pick SynapseClaw

- **Lean by default:** single Rust binary, fast startup, low memory footprint.
- **Secure by design:** pairing, strict sandboxing, explicit allowlists, workspace scoping.
- **Composable:** providers, channels, tools, memory, and tunnels remain swappable.
- **Usable as both local runtime and control plane:** single-agent workbench and broker-managed multi-agent operation share one runtime model.
- **No lock-in:** OpenAI-compatible provider support + pluggable custom endpoints.

## What makes SynapseClaw different

SynapseClaw is not only a local chat wrapper around an LLM. The project is deliberately moving toward a small-footprint runtime that can operate in two modes without splitting into two products:

- **Focused single-agent mode:** one daemon, one agent, one workbench for tuning prompts, tools, memory, logs, channels, and runtime behavior.
- **Brokered multi-agent mode:** one broker dashboard, many agent daemons, secure IPC, selected-agent workbench pages through the broker, and one operator entrypoint instead of a pile of ports and SSH tunnels.

The main differences today are practical:

- **Brokered agent families:** agents can register with a broker, talk over an IPC bus, and be operated from one control plane instead of being treated as isolated chatbots.
- **Trust-aware IPC:** inter-agent messaging is not a best-effort chat hack; it has trust levels, directional ACLs, quarantine handling, revocation, signing, and delivery controls.
- **Operator-first UI:** the same frontend shell serves both a local agent workbench and a broker mode with fleet pages, selected-agent pages, activity tracing, provisioning, and topology views.
- **Durable sessions:** web chat is not throwaway browser state anymore; sessions, runs, summaries, and goals are becoming first-class runtime objects.
- **Pragmatic infrastructure:** low RAM, single-binary Rust runtime, swappable providers/channels/tools, and deployability on cheap hardware remain non-negotiable.

## Where SynapseClaw is going

The current direction is not “add more random integrations.” It is to make the system easier to operate, easier to reason about, and more modular:

- **Make multi-agent operations usable:** one broker, one dashboard, selected-agent drill-down, better traceability between IPC, spawn runs, chats, channels, and cron.
- **Clean up fleet topology:** separate declared policy topology from observed traffic, hide ephemeral clutter by default, and add blueprint-level views for larger fleets.
- **Move to an obvious modular core:** capability-driven channels, fixed transport boundaries, one conversation store contract, and one run substrate instead of logic scattered across gateway, tools, channels, cron, and agent loop.
- **Make memory explicit:** treat working memory, session memory, and long-term memory as separate ports instead of accidental side effects of whichever subsystem happens to store state.
- **Keep external coding engines as bounded workers:** if we later integrate tools like Codex or Claude Code, they should arrive as specialized execution workers behind a clean port, not as a second application core.

For the fork-specific architecture plans, execution checklists, and roadmap details, start at [`docs/fork/README.md`](docs/fork/README.md).
For the latest updates and release notes, see [`docs/fork/news.md`](docs/fork/news.md).

## Benchmark Snapshot (SynapseClaw vs OpenClaw, Reproducible)

Local machine quick benchmark (macOS arm64, Feb 2026) normalized for 0.8GHz edge hardware.

|                           | OpenClaw      | NanoBot        | PicoClaw        | SynapseClaw 🦀          |
| ------------------------- | ------------- | -------------- | --------------- | -------------------- |
| **Language**              | TypeScript    | Python         | Go              | **Rust**             |
| **RAM**                   | > 1GB         | > 100MB        | < 10MB          | **< 5MB**            |
| **Startup (0.8GHz core)** | > 500s        | > 30s          | < 1s            | **< 10ms**           |
| **Binary Size**           | ~28MB (dist)  | N/A (Scripts)  | ~8MB            | **~8.8 MB**          |
| **Cost**                  | Mac Mini $599 | Linux SBC ~$50 | Linux Board $10 | **Any hardware $10** |

> Notes: SynapseClaw results are measured on release builds using `/usr/bin/time -l`. OpenClaw requires Node.js runtime (typically ~390MB additional memory overhead), while NanoBot requires Python runtime. PicoClaw and SynapseClaw are static binaries. The RAM figures above are runtime memory; build-time compilation requirements are higher.

<p align="center">
  <img src="docs/assets/synapseclaw-comparison.jpeg" alt="SynapseClaw vs OpenClaw Comparison" width="800" />
</p>

### Reproducible local measurement

Benchmark claims can drift as code and toolchains evolve, so always measure your current build locally:

```bash
cargo build --release
ls -lh target/release/synapseclaw

/usr/bin/time -l target/release/synapseclaw --help
/usr/bin/time -l target/release/synapseclaw status
```

Example sample (macOS arm64, measured on February 18, 2026):

- Release binary size: `8.8MB`
- `synapseclaw --help`: about `0.02s` real time, ~`3.9MB` peak memory footprint
- `synapseclaw status`: about `0.01s` real time, ~`4.1MB` peak memory footprint

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

Or skip the steps above and install everything (system deps, Rust, SynapseClaw) in a single command:

```bash
curl -LsSf https://raw.githubusercontent.com/panviktor/synapseclaw/master/install.sh | bash
```

#### Compilation resource requirements

Building from source needs more resources than running the resulting binary:

| Resource       | Minimum | Recommended |
| -------------- | ------- | ----------- |
| **RAM + swap** | 2 GB    | 4 GB+       |
| **Free disk**  | 6 GB    | 10 GB+      |

If your host is below the minimum, use pre-built binaries:

```bash
./install.sh --prefer-prebuilt
```

To require binary-only install with no source fallback:

```bash
./install.sh --prebuilt-only
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
# Recommended: clone then run local bootstrap script
git clone https://github.com/panviktor/synapseclaw.git
cd synapseclaw
./install.sh

# Optional: bootstrap dependencies + Rust on fresh machines
./install.sh --install-system-deps --install-rust

# Optional: pre-built binary first (recommended on low-RAM/low-disk hosts)
./install.sh --prefer-prebuilt

# Optional: binary-only install (no source build fallback)
./install.sh --prebuilt-only

# Optional: run onboarding in the same flow
./install.sh --api-key "sk-..." --provider openrouter [--model "openrouter/auto"]

# Optional: run bootstrap + onboarding fully in Docker-compatible mode
./install.sh --docker

# Optional: force Podman as container CLI
SYNAPSECLAW_CONTAINER_CLI=podman ./install.sh --docker

# Optional: in --docker mode, skip local image build and use local tag or pull fallback image
./install.sh --docker --skip-build
```

Remote one-liner (review first in security-sensitive environments):

```bash
curl -fsSL https://raw.githubusercontent.com/panviktor/synapseclaw/master/install.sh | bash
```

Details: [`docs/setup-guides/one-click-bootstrap.md`](docs/setup-guides/one-click-bootstrap.md) (toolchain mode may request `sudo` for system packages).

### Pre-built binaries

Release assets are published for:

- Linux: `x86_64`, `aarch64`, `armv7`
- macOS: `x86_64`, `aarch64`
- Windows: `x86_64`

Download the latest assets from:
<https://github.com/panviktor/synapseclaw/releases/latest>

Example (ARM64 Linux):

```bash
curl -fsSLO https://github.com/panviktor/synapseclaw/releases/latest/download/synapseclaw-aarch64-unknown-linux-gnu.tar.gz
tar xzf synapseclaw-aarch64-unknown-linux-gnu.tar.gz
install -m 0755 synapseclaw "$HOME/.cargo/bin/synapseclaw"
```

```bash
git clone https://github.com/panviktor/synapseclaw.git
cd synapseclaw
cargo build --release --locked
cargo install --path . --force --locked

# Ensure ~/.cargo/bin is in your PATH
export PATH="$HOME/.cargo/bin:$PATH"

# Quick setup (no prompts, optional model specification)
synapseclaw onboard --api-key sk-... --provider openrouter [--model "openrouter/auto"]

# Or guided wizard
synapseclaw onboard

# If config.toml already exists and you intentionally want to overwrite it
synapseclaw onboard --force

# Or quickly repair channels/allowlists only
synapseclaw onboard --channels-only

# Chat
synapseclaw agent -m "Hello, SynapseClaw!"

# Interactive mode
synapseclaw agent

# Start the gateway (webhook server)
synapseclaw gateway                # default: 127.0.0.1:42617
synapseclaw gateway --port 0       # random port (security hardened)

# Start full autonomous runtime
synapseclaw daemon

# Check status
synapseclaw status
synapseclaw auth status

# Generate shell completions (stdout only, safe to source directly)
source <(synapseclaw completions bash)
synapseclaw completions zsh > ~/.zfunc/_synapseclaw

# Run system diagnostics
synapseclaw doctor

# Check channel health
synapseclaw channel doctor

# Bind a Telegram identity into allowlist
synapseclaw channel bind-telegram 123456789

# Get integration setup details
synapseclaw integrations info Telegram

# Note: Channels (Telegram, Discord, Slack) require daemon to be running
# synapseclaw daemon

# Manage background service
synapseclaw service install
synapseclaw service status
synapseclaw service restart

# On Alpine (OpenRC): sudo synapseclaw service install

# Migrate memory from OpenClaw (safe preview first)
synapseclaw migrate openclaw --dry-run
synapseclaw migrate openclaw
```

> **Dev fallback (no global install):** prefix commands with `cargo run --release --` (example: `cargo run --release -- status`).

## Subscription Auth (OpenAI Codex / Claude Code)

SynapseClaw now supports subscription-native auth profiles (multi-account, encrypted at rest).

- Store file: `~/.synapseclaw/auth-profiles.json`
- Encryption key: `~/.synapseclaw/.secret_key`
- Profile id format: `<provider>:<profile_name>` (example: `openai-codex:work`)

OpenAI Codex OAuth (ChatGPT subscription):

```bash
# Recommended on servers/headless
synapseclaw auth login --provider openai-codex --device-code

# Browser/callback flow with paste fallback
synapseclaw auth login --provider openai-codex --profile default
synapseclaw auth paste-redirect --provider openai-codex --profile default

# Check / refresh / switch profile
synapseclaw auth status
synapseclaw auth refresh --provider openai-codex --profile default
synapseclaw auth use --provider openai-codex --profile work
```

Claude Code / Anthropic setup-token:

```bash
# Paste subscription/setup token (Authorization header mode)
synapseclaw auth paste-token --provider anthropic --profile default --auth-kind authorization

# Alias command
synapseclaw auth setup-token --provider anthropic --profile default
```

Run the agent with subscription auth:

```bash
synapseclaw agent --provider openai-codex -m "hello"
synapseclaw agent --provider openai-codex --auth-profile openai-codex:work -m "hello"

# Anthropic supports both API key and auth token env vars:
# ANTHROPIC_AUTH_TOKEN, ANTHROPIC_OAUTH_TOKEN, ANTHROPIC_API_KEY
synapseclaw agent --provider anthropic -m "hello"
```

## Collaboration & Docs

Start from the docs hub for a task-oriented map:

- Documentation hub: [`docs/README.md`](docs/README.md)
- Unified docs TOC: [`docs/SUMMARY.md`](docs/SUMMARY.md)
- Commands reference: [`docs/reference/cli/commands-reference.md`](docs/reference/cli/commands-reference.md)
- Config reference: [`docs/reference/api/config-reference.md`](docs/reference/api/config-reference.md)
- Providers reference: [`docs/reference/api/providers-reference.md`](docs/reference/api/providers-reference.md)
- Channels reference: [`docs/reference/api/channels-reference.md`](docs/reference/api/channels-reference.md)
- Operations runbook: [`docs/ops/operations-runbook.md`](docs/ops/operations-runbook.md)
- Troubleshooting: [`docs/ops/troubleshooting.md`](docs/ops/troubleshooting.md)
- Docs inventory/classification: [`docs/maintainers/docs-inventory.md`](docs/maintainers/docs-inventory.md)
- PR/Issue triage snapshot (as of February 18, 2026): [`docs/maintainers/project-triage-snapshot-2026-02-18.md`](docs/maintainers/project-triage-snapshot-2026-02-18.md)

Core collaboration references:

- Documentation hub: [docs/README.md](docs/README.md)
- Fork roadmap and architecture plans: [docs/fork/README.md](docs/fork/README.md)
- Documentation template: [docs/contributing/doc-template.md](docs/contributing/doc-template.md)
- Documentation change checklist: [docs/README.md#4-documentation-change-checklist](docs/README.md#4-documentation-change-checklist)
- Channel configuration reference: [docs/reference/api/channels-reference.md](docs/reference/api/channels-reference.md)
- Matrix encrypted-room operations: [docs/security/matrix-e2ee-guide.md](docs/security/matrix-e2ee-guide.md)
- Contribution guide: [CONTRIBUTING.md](CONTRIBUTING.md)
- PR workflow policy: [docs/contributing/pr-workflow.md](docs/contributing/pr-workflow.md)
- Reviewer playbook (triage + deep review): [docs/contributing/reviewer-playbook.md](docs/contributing/reviewer-playbook.md)
- Security disclosure policy: [SECURITY.md](SECURITY.md)

For deployment and runtime operations:

- Network deployment guide: [docs/ops/network-deployment.md](docs/ops/network-deployment.md)
- Proxy agent playbook: [docs/ops/proxy-agent-playbook.md](docs/ops/proxy-agent-playbook.md)

## Support SynapseClaw

If SynapseClaw helps your work and you want to support ongoing development, you can donate here:

<a href="https://buymeacoffee.com/argenistherose"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-Donate-yellow.svg?style=for-the-badge&logo=buy-me-a-coffee" alt="Buy Me a Coffee" /></a>

### 🙏 Special Thanks

A heartfelt thank you to the communities and institutions that inspire and fuel this open-source work:

- **Harvard University** — for fostering intellectual curiosity and pushing the boundaries of what's possible.
- **MIT** — for championing open knowledge, open source, and the belief that technology should be accessible to everyone.
- **Sundai Club** — for the community, the energy, and the relentless drive to build things that matter.
- **The World & Beyond** 🌍✨ — to every contributor, dreamer, and builder out there making open source a force for good. This is for you.

We're building in the open because the best ideas come from everywhere. If you're reading this, you're part of it. Welcome. 🦀❤️

<!-- BEGIN:RECENT_CONTRIBUTORS -->
<!-- END:RECENT_CONTRIBUTORS -->

## License

SynapseClaw is dual-licensed for maximum openness and contributor protection:

| License | Use case |
|---|---|
| [MIT](LICENSE-MIT) | Open-source, research, academic, personal use |
| [Apache 2.0](LICENSE-APACHE) | Patent protection, institutional, commercial deployment |

You may choose either license. **Contributors automatically grant rights under both** — see [CLA.md](docs/contributing/cla.md) for the full contributor agreement.

### Trademark

The **SynapseClaw** name and logo are trademarks of SynapseClaw Labs. This license does not grant permission to use them to imply endorsement or affiliation. See [TRADEMARK.md](docs/maintainers/trademark.md) for permitted and prohibited uses.

### Contributor Protections

- You **retain copyright** of your contributions
- **Patent grant** (Apache 2.0) shields you from patent claims by other contributors
- Your contributions are **permanently attributed** in commit history and [NOTICE](NOTICE)
- No trademark rights are transferred by contributing

## Contributing

New to SynapseClaw? Look for issues labeled [`good first issue`](https://github.com/panviktor/synapseclaw/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) — see our [Contributing Guide](CONTRIBUTING.md#first-time-contributors) for how to get started.

See [CONTRIBUTING.md](CONTRIBUTING.md) and [CLA.md](docs/contributing/cla.md). Implement a trait, submit a PR:

- CI workflow guide: [docs/contributing/ci-map.md](docs/contributing/ci-map.md)
- New `Provider` → `src/providers/`
- New `Channel` → `src/channels/`
- New `Observer` → `src/observability/`
- New `Tool` → `src/tools/`
- New `Memory` → `src/memory/`
- New `Tunnel` → `src/tunnel/`
- New `Skill` → `~/.synapseclaw/workspace/skills/<name>/`

---

**SynapseClaw** — Zero overhead. Zero compromise. Deploy anywhere. Swap anything. 🦀

## Contributors

<a href="https://github.com/panviktor/synapseclaw/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=panviktor/synapseclaw" alt="SynapseClaw contributors" />
</a>

## Star History

<p align="center">
  <a href="https://www.star-history.com/#panviktor/synapseclaw&type=date&legend=top-left">
    <picture>
     <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=panviktor/synapseclaw&type=date&theme=dark&legend=top-left" />
     <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=panviktor/synapseclaw&type=date&legend=top-left" />
     <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=panviktor/synapseclaw&type=date&legend=top-left" />
    </picture>
  </a>
</p>
