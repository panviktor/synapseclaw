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

