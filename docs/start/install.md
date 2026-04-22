# Install

For normal users, the supported first path is:

1. install the binary
2. run `synapseclaw onboard`
3. connect one provider and one channel
4. optionally install the background service

`install.sh` is prebuilt-first and should be the default entrypoint when you are not developing SynapseClaw itself.

```bash
./install.sh
```

If you are building from source, use:

```bash
cargo build --release --features channel-matrix
```

Local configuration normally lives under `~/.synapseclaw/`. Secrets should live outside tracked config files:

- Linux: `~/.config/systemd/user/synapseclaw.env`
- macOS: `~/.synapseclaw/synapseclaw.env`

After the binary is available, prefer `synapseclaw onboard` over manual config editing for the first run.
