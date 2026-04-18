# Deploy

Build the release binary with Matrix channel support:

```bash
cargo build --release --features channel-matrix
```

Stop the service fleet, install the binary, restart, and verify service state:

```bash
systemctl --user stop synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
cp target/release/synapseclaw ~/.cargo/bin/synapseclaw
systemctl --user start synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
systemctl --user is-active synapseclaw.service synapseclaw@{copywriter,marketing-lead,news-reader,publisher,trend-aggregator}.service
```

Then check gateway health:

```bash
for port in 42617 42618 42619 42620 42621 42622; do
  curl -fsS "http://127.0.0.1:${port}/health" || exit 1
done
```

