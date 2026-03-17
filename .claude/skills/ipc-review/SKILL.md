---
name: ipc-review
description: "Audit IPC code for security and correctness issues. Checks ACL consistency, audit semantics, quarantine/promote flow, sequence integrity, PromptGuard integration, and structured output. Use when reviewing IPC changes, preparing for merge, or after modifying src/gateway/ipc.rs, src/tools/agents_ipc.rs, or src/security/pairing.rs."
user-invocable: true
---

# IPC Security Review

Perform a structured audit of the IPC subsystem. Read the code, check invariants, report findings with severity and refs.

## What to check

Read these files (in parallel where possible):
- `src/gateway/ipc.rs` — broker handlers, IpcDb, ACL validation
- `src/gateway/agent_registry.rs` — agent registry, health polling
- `src/gateway/chat_db.rs` — chat session persistence
- `src/gateway/provisioning.rs` — agent provisioning from UI
- `src/tools/agents_ipc.rs` — IPC tool implementations
- `src/security/pairing.rs` — token auth, TokenMetadata
- `src/security/execution.rs` — execution profiles, fail-closed sandbox
- `src/security/identity.rs` — Ed25519 agent identity, key registration
- `src/security/prompt_guard.rs` — PromptGuard payload scanning
- `src/gateway/mod.rs` — AppState, route registration, PromptGuard init
- `src/config/schema.rs` — AgentsIpcConfig, IpcPromptGuardConfig

Then check each category below. For each finding, report:
- **Severity**: High / Medium / Low
- **Description**: what's wrong
- **Refs**: file:line references
- **Impact**: what can go wrong in production

### 1. ACL Consistency

Verify these invariants hold in `validate_send()`:
- Rule 0: only text/task/result/query accepted as valid kinds
- Rule 1: L4 can only send `kind=text`
- Rule 2: task only goes downward (`to_level > from_level`)
- Rule 3: result requires correlated session_id with existing task/query
- Rule 4: L4↔L4 lateral forbidden
- Rule 5: L3 lateral text requires allowlisting in `lateral_text_pairs`

Check that ACL validation happens BEFORE any DB write.

### 2. Audit Semantics

- Every reject path must emit an audit event with `allowed = false`
- `IpcSend` emitted on successful send
- `IpcReceived` emitted on inbox fetch
- `IpcBlocked` emitted on ACL denial, PromptGuard block, LeakDetector block
- `IpcRateLimited` emitted on rate limit hit
- `IpcAdminAction` emitted on admin operations (revoke, disable, quarantine, downgrade, promote)
- `IpcLeakDetected` emitted on credential leak detection

### 3. Quarantine / Promote Flow

- `fetch_inbox(quarantine=true)` must NOT mark messages as `read=1`
- `handle_admin_ipc_promote` must validate: not already promoted, not already read, target agent exists, message is from L4
- Promoted messages appear in normal inbox (not quarantine)
- Sequence integrity check applies to promoted inserts too

### 4. Sequence Integrity

- `check_seq_integrity()` called in both `insert_message` and `insert_promoted_message`
- `IpcInsertError::SequenceViolation` surfaced as HTTP 409 with `sequence_violation` code
- Corruption detection: if `next_seq()` returns value ≤ max existing seq for the pair, it's caught

### 5. PromptGuard + LeakDetector

- PromptGuard scans payload BEFORE insert (not after)
- `GuardAction::from_str("sanitize")` maps to `Block` (sanitize not implemented)
- Exempt levels from config are respected (default: L0, L1)
- LeakDetector scans both `send` and `state_set` payloads
- Both scanners emit audit events on detection

### 6. Structured Output

- `trust_warning` field populated for L3+ senders
- `quarantined` field set for quarantine lane messages
- L4 messages get explicit "QUARANTINE" prefix in trust_warning
- Messages returned as structured JSON, not raw text

### 7. Session Limits

- `session_message_count()` checked against `session_max_exchanges` config
- Escalation auto-creates `ESCALATION_KIND` message to coordinator
- `ESCALATION_KIND` and `PROMOTED_KIND` not in `VALID_KINDS` (can't be sent directly)

### 8. Token / Pairing Security

- `TokenMetadata` correctly resolves agent_id, trust_level, role from bearer token
- Agent cannot claim a trust_level different from what broker assigned
- `revoke_by_agent_id` removes from both `paired_tokens` and `token_metadata`
- Admin endpoints are localhost-only (`require_localhost` guard)

## Output format

```
## IPC Security Review — {date}

### Findings

1. **{severity}** — {description}
   Refs: {file:line, file:line}
   Impact: {what can go wrong}

2. ...

### What passed
- {category}: {brief confirmation}

### Validation
- cargo fmt: {pass/fail}
- cargo clippy: {pass/fail}
- cargo test (IPC): {pass/fail, count}
```

Run `cargo test gateway::ipc::tests` and `cargo test tools::agents_ipc::tests` as part of the review. Report results.

If $ARGUMENTS is provided, focus only on that category (e.g., `/ipc-review acl` checks only ACL consistency).
