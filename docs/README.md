# Synapseclaw Documentation

Synapseclaw is a local agent runtime for working through chat, web UI, channels, tools, memory, and reusable skills. The documentation is organized by the path a reader is likely to take: start the system, use it, extend it, operate it, understand its architecture, or look up exact reference material.

The Skills documentation is the most detailed user-facing area because that subsystem is already beta-ready. Other user-facing areas are intentionally shorter while their UX continues to evolve.

## Start Here

| I want to... | Read this |
| --- | --- |
| Understand what Synapseclaw is | [start/what-is-synapseclaw.md](start/what-is-synapseclaw.md) |
| Build and run it for the first time | [start/first-run.md](start/first-run.md) |
| Create and use a skill | [use/skills/quickstart.md](use/skills/quickstart.md) |
| Use the web UI | [use/web-ui.md](use/web-ui.md) |
| Deploy the local service fleet | [operate/deploy.md](operate/deploy.md) |
| Add or maintain tools | [extend/add-tool.md](extend/add-tool.md) |
| Understand the architecture | [understand/architecture.md](understand/architecture.md) |
| Look up exact commands or APIs | [reference/README.md](reference/README.md) |

## Collections

- [Start](start/what-is-synapseclaw.md) - first-run material for new users.
- [Use](use/README.md) - user-facing workflows, with detailed Skills guidance.
- [Extend](extend/README.md) - developer guidance for tools, skills, channels, providers, and APIs.
- [Operate](operate/README.md) - deployment, services, health checks, logs, backup, and security.
- [Understand](understand/README.md) - architecture notes for memory, compaction, skills, channels, and replay privacy.
- [Reference](reference/README.md) - exact CLI, API, config, lifecycle, and contract material.
- [Deprecated](deprecated/README.md) - historical plans, audits, and fork-era implementation notes.

## Current Stability

Skills are the primary stable workflow feature documented in depth. Chat, web UI, Matrix, memory, and automations are usable but still evolving, so their user pages focus on accurate concepts and safe entry points rather than long tutorials.

Historical phase plans and audits have been moved under [deprecated/](deprecated/README.md). They remain useful for development archaeology, but they are not current user instructions.
