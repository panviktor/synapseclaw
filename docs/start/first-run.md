# First Run

The intended first run is:

```bash
synapseclaw onboard
```

The default onboarding flow is optimized for one provider plus one real channel. Today the smoothest first channel is Telegram; Matrix is the main self-hosted option.

At the end of onboarding, SynapseClaw can:

- write secrets into the local env file
- save `config.toml`
- optionally install and start the background service

For a production-like local fleet, use the deployment flow in [operate/deploy.md](../operate/deploy.md). Once the runtime answers a simple request, the fastest useful workflow to try is [Skills Quickstart](../use/skills/quickstart.md).
