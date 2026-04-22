# Health And Logs

Use `/health` to confirm that the gateway and runtime components are alive. A service can be `active` before HTTP health is ready, so retry briefly after restart before treating it as failed.

Useful checks:

```bash
systemctl --user status synapseclaw.service --no-pager
journalctl --user -u synapseclaw.service -n 120 --no-pager
synapseclaw skills health --limit 5 --trace-limit 20
synapseclaw skills candidates --limit 3
```

The Skills commands are good smoke tests because they exercise gateway, memory, governance, and formatting paths.

Runtime diagnostics now also surface compact usage, pressure, watchdog, and implicit-memory signals through the shared diagnostics output. Expect lines such as:

- `Runtime usage: ...`
- `Runtime usage pressure: ...`
- `Runtime watchdog: ...`
- `Runtime decision note: implicit_memory_recall / ...`

Those lines are intentionally bounded. They are for operator inspection, not a raw dump of memory payloads or replay history.
