# Install

Synapseclaw is built from the Rust workspace. For the local feature set used by the current service fleet, build with Matrix channel support:

```bash
cargo build --release --features channel-matrix
```

Local configuration normally lives under `~/.synapseclaw/`, while operational secrets should live outside the repository, usually in a user systemd environment file. Provider and channel setup is still evolving, so keep first installs minimal and move to [operate/config.md](../operate/config.md) once the basic binary works.

