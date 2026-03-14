---
name: ipc-smoke
description: "Smoke test IPC endpoints against a running broker. Tests pairing, agent listing, message send/receive, shared state, ACL denial, quarantine, and admin operations. Use when the user says 'smoke test IPC', 'проверь IPC', 'test the broker', or wants to verify IPC is working end-to-end."
user-invocable: true
---

# IPC Smoke Test

Run end-to-end smoke tests against a running ZeroClaw broker.

## Prerequisites

Check that the gateway is running:
```bash
curl -sf http://127.0.0.1:42617/health
```

If not running, tell the user to start it (`zeroclaw daemon` or `zeroclaw gateway`).

Check IPC is enabled:
```bash
curl -sf http://127.0.0.1:42617/health | jq .
```

## Test sequence

Use the quickstart guide as reference: `docs/fork/ipc-quickstart.md`

### 1. Pairing

Generate paircodes for two agents:

```bash
# L1 coordinator
curl -sS -X POST http://127.0.0.1:42617/admin/paircode/new \
  -H 'Content-Type: application/json' \
  -d '{"agent_id": "test-opus", "trust_level": 1, "role": "coordinator"}'

# L3 worker
curl -sS -X POST http://127.0.0.1:42617/admin/paircode/new \
  -H 'Content-Type: application/json' \
  -d '{"agent_id": "test-worker", "trust_level": 3, "role": "worker"}'
```

Exchange paircodes for tokens:
```bash
TOKEN_L1=$(curl -sS -X POST http://127.0.0.1:42617/pair \
  -H "X-Pairing-Code: <code1>" | jq -r '.token')

TOKEN_L3=$(curl -sS -X POST http://127.0.0.1:42617/pair \
  -H "X-Pairing-Code: <code2>" | jq -r '.token')
```

**Pass**: both return tokens. **Fail**: 4xx/5xx response.

### 2. Agent listing

```bash
curl -sS http://127.0.0.1:42617/api/ipc/agents \
  -H "Authorization: Bearer $TOKEN_L1" | jq .
```

**Pass**: returns JSON array containing test-opus and test-worker. **Fail**: empty or error.

### 3. Send message (L1 → L3 task)

```bash
curl -sS -X POST http://127.0.0.1:42617/api/ipc/send \
  -H "Authorization: Bearer $TOKEN_L1" \
  -H 'Content-Type: application/json' \
  -d '{"to": "test-worker", "kind": "task", "payload": "smoke test task"}' | jq .
```

**Pass**: 200 with message_id. **Fail**: 4xx.

### 4. Check inbox (as L3)

```bash
curl -sS http://127.0.0.1:42617/api/ipc/inbox \
  -H "Authorization: Bearer $TOKEN_L3" | jq .
```

**Pass**: returns message with `from_agent: "test-opus"`, `kind: "task"`. **Fail**: empty or error.

### 5. ACL denial (L3 → L1 task)

```bash
curl -sS -X POST http://127.0.0.1:42617/api/ipc/send \
  -H "Authorization: Bearer $TOKEN_L3" \
  -H 'Content-Type: application/json' \
  -d '{"to": "test-opus", "kind": "task", "payload": "should be denied"}'
```

**Pass**: 403 with `task_upward_denied`. **Fail**: 200 (ACL broken).

### 6. Shared state

```bash
# Set (L1 writes global:*)
curl -sS -X POST http://127.0.0.1:42617/api/ipc/state \
  -H "Authorization: Bearer $TOKEN_L1" \
  -H 'Content-Type: application/json' \
  -d '{"key": "global:smoke:test", "value": "ok"}'

# Get (L3 reads)
curl -sS "http://127.0.0.1:42617/api/ipc/state?key=global:smoke:test" \
  -H "Authorization: Bearer $TOKEN_L3" | jq .
```

**Pass**: value is "ok". **Fail**: missing or error.

### 7. State ACL denial (L3 writes global:*)

```bash
curl -sS -X POST http://127.0.0.1:42617/api/ipc/state \
  -H "Authorization: Bearer $TOKEN_L3" \
  -H 'Content-Type: application/json' \
  -d '{"key": "global:smoke:hack", "value": "nope"}'
```

**Pass**: 403 with `global_denied`. **Fail**: 200 (ACL broken).

### 8. Admin operations

```bash
# List agents (admin, localhost only)
curl -sS http://127.0.0.1:42617/admin/ipc/agents | jq .
```

**Pass**: returns full agent list with metadata.

### Cleanup

Revoke test agents:
```bash
curl -sS -X POST http://127.0.0.1:42617/admin/ipc/revoke \
  -H 'Content-Type: application/json' \
  -d '{"agent_id": "test-opus"}'

curl -sS -X POST http://127.0.0.1:42617/admin/ipc/revoke \
  -H 'Content-Type: application/json' \
  -d '{"agent_id": "test-worker"}'
```

## Output

```
## IPC Smoke Test — {date}

| # | Test | Result | Detail |
|---|------|--------|--------|
| 1 | Pairing | ✓/✗ | ... |
| 2 | Agent listing | ✓/✗ | ... |
| 3 | Send (L1→L3 task) | ✓/✗ | ... |
| 4 | Inbox (L3) | ✓/✗ | ... |
| 5 | ACL denial (L3→L1 task) | ✓/✗ | ... |
| 6 | Shared state | ✓/✗ | ... |
| 7 | State ACL denial | ✓/✗ | ... |
| 8 | Admin listing | ✓/✗ | ... |

**Result: {N}/8 passed**
```

## Arguments

- No args: full smoke test suite
- `quick`: only tests 1-4 (pairing + send/receive)
- `acl`: only ACL denial tests (5, 7)
- `no-cleanup`: skip revoking test agents at the end
