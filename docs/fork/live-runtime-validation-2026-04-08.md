# Live Runtime Validation — 2026-04-08

Scope:
- Everyday-flow validation for Phase 4.8 / 4.9 on the live gateway daemon.
- Main focus: context mixing, delivery target recall, procedural learning, failure learning.

Method:
- Drive the running daemon through `dev/gateway-chat-harness`.
- Validate tool traces, final answers, and selected downstream state/projections.
- Do not record bearer tokens or secrets in this log.

## Results

### 1. Post-Restart Report To Matrix
Status: pass with latency caveat
Notes:
- Cross-session recall works: a fresh session recalled the requested restart report shape and the Matrix room id `!OL9Ltf2mV131bsr1u1:matrix.capeofcryptohope.com`.
- Dry-run setup turn returned `rpc_result.aborted = true` once, with no assistant/tool events captured.
- A later live retry did deliver `SYNAPSECLAW RESTART REPORT TEST 2026-04-08` to the configured Matrix room.
- Delivery works, but the path is still noisy/unstable: the agent often digs through workspace/session artifacts before sending.

### 2. Available Updates Workflow
Status: partial
Notes:
- Final workflow answer was sensible: `apt update` -> `apt list --upgradable` -> count -> skim important packages.
- Turn 1 was heavy (`9` tool calls / `9` tool results) for a routine task.
- Turn 2 still showed noisy duplication: `duplicate_tool_calls = 1`, `duplicate_tool_results = 1`.
- The agent can explain the workflow afterwards, but the runtime still overworks simple ops checks.

### 3. City Preference To Weather In New Session
Status: fail
Notes:
- The harmless preference-setting turn (`default city = Berlin`) originally crashed the main daemon because core-memory trimming sliced UTF-8 by byte index in `surrealdb_adapter.rs`.
- That crash was fixed locally during validation, but the retry still did not complete the turn in time.
- The gateway persisted the user turn, then spent tens of seconds in `hybrid_search` and repeated `embed_document` work without ever persisting an assistant reply before the harness timed out.
- This is a completion-path/runtime issue, not a weather-specific logic failure.

### 4. Service Check With Contradiction
Status: fail
Notes:
- The setup turn did the right *kind* of memory update (`core_memory_update` with the contradiction-aware rule) but never completed the turn within timeout.
- The follow-up turn `Проверь состояние synapseclaw.service, его только что перезапускали.` also timed out without a single assistant reply.
- This confirms the delayed/blocked completion problem is systemic and not isolated to the weather tool path.
- The setup turn also showed contamination in retrieval: it pulled `Atlas` / `Borealis` service-check memories into an unrelated local service workflow.

### 5. Topic Isolation: Ops vs Marketing
Status: fail
Notes:
- The setup turn again timed out after file bootstrap (`SOUL.md`, `USER.md`), `memory_recall`, and `core_memory_update`.
- The marketing follow-up `Сделай короткий анонс релиза для канала.` timed out immediately after a single `memory_recall` tool call.
- No evidence of a clean `marketing-lead` delegation path was observed in this validation pass because the turn never reached a stable assistant completion.

## Cross-Cutting Findings

- The primary live regression is now **run completion instability on innocuous memory-setting turns**. Storing preferences/instructions can block or time out before the assistant reply is emitted.
- This is broader than one tool or one scenario:
  - restart-report setup
  - city/weather preference setup
  - contradiction-aware service-check setup
  - topic-isolation setup
  all exhibit the same delayed or missing completion behavior.
- `memory_recall` and cross-session persistence work, but retrieval still contaminates local tasks with unrelated prior contexts (`Atlas`, `Borealis`, older procedural precedents).
- Duplicate/noisy tool activity remains present on simple operational tasks.
- Matrix delivery itself works; the failure is in the planner/runtime path leading up to it.
- A real runtime crash was found during validation:
  - UTF-8-unsafe trimming in Surreal core-memory append path
  - fixed locally during this validation pass before continuing

## Tool Migration Backlog Surfaced By Validation

Observed tool usage across the live validation artifacts:

| Tool | Observed calls | Current typed-fact status | First batch? | Why it matters |
|---|---:|---|---|---|
| `content_search` | 14 | Partial | Yes | Dominates workspace archaeology loops; current search fact is too shallow to distinguish productive search from noisy bootstrap/search churn. |
| `file_read` | 13 | Partial | Yes | Emits only a generic file read resource fact; live failures show it being overused for `SOUL.md` / `USER.md` bootstrap and unrelated file digging. |
| `shell` | 11 | Partial | Yes | Critical for service checks and update flows, but facts are still mostly generic focus facts instead of structured operational signals. |
| `memory_recall` | 5 | Partial | Yes | Central to context contamination; currently emits only generic focus entities instead of a richer recall/search signal. |
| `glob_search` | 4 | Partial | Yes | Participates in workspace archaeology loops; search facts need stronger locators and stronger downranking of bootstrap-only matches. |
| `core_memory_update` | 3 | Partial | Yes | Instruction and preference turns depend on it, but current facts only mark the core block/action and lose most typed semantics. |
| `user_profile` | 2 | Good baseline | No | Already emits typed user-profile facts; keep as a reference implementation. |
| `session_search` | 1 | Good baseline | No | Already emits typed session search facts; useful as a reference for search-tool migration. |
| `precedent_search` | 1 | Good baseline | No | Already emits typed precedent search facts; useful as a reference for retrieval-tool migration. |
| `file_write` | 1 | Good baseline | No | Already emits typed write resource facts; lower priority for current regressions. |
| `file_edit` | 1 | Good baseline | No | Already emits typed edit resource facts; lower priority for current regressions. |

Migration coverage proxy for the tool layer:

- `crates/adapters/tools/src` source files: `64`
- source files with `extract_facts(...)` or `execute_with_facts(...)`: `25`
- remaining source files without typed-fact hooks: `39`

The first batch is intentionally validation-driven, not theoretical:

1. `content_search`
2. `file_read`
3. `shell`
4. `memory_recall`
5. `glob_search`
6. `core_memory_update`

See [tool-fact-porting-instructions.md](/home/protosik00/synapseclaw/docs/fork/tool-fact-porting-instructions.md) for the detailed porting guide and acceptance criteria.

## Follow-Up Validation After Runtime And Tool Fixes

Applied during the same validation pass:

- moved `chat.send` success bookkeeping off the synchronous WebSocket completion path
- deduplicated live/persisted tool events by structural signature
- hardened first-batch tool facts without semantic string heuristics
- fixed `user_profile` to accept `clear_fields: null`
- restored user-systemd bus environment in `shell` subprocesses after `env_clear()`

### 6. Topic Isolation Retest
Status: pass
Notes:
- The setup turn now completes successfully instead of timing out.
- The marketing follow-up cleanly delegates to `marketing-lead`.
- No duplicate tool calls or duplicate tool results were observed.

### 7. City Preference Retest
Status: pass
Notes:
- The setup turn now completes successfully instead of timing out.
- The earlier `user_profile` failure on `clear_fields = null` is gone.
- The follow-up weather turn returns the correct default city (`Berlin`) with no duplicate tool events.
- Remaining quality issue from that older pass: the planner still bootstrapped through workspace docs before acting.

### 8. Service Check Retest
Status: pass with planner caveat
Notes:
- Both the setup turn and the follow-up turn now complete successfully.
- No duplicate tool calls or duplicate tool results were observed.
- Restoring user-bus environment fixed `systemctl --user is-active synapseclaw.service`; it now returns `active`.
- Remaining quality issue: the planner still first attempts an overcomplicated multi-line shell script that security policy correctly blocks, then recovers via simpler commands.

### 9. Provider-Context Compaction Retest
Status: partial pass
Notes:
- After switching the live agent loop to compact provider-facing history, ordinary memory-setting turns no longer replay the old bootstrap-heavy path.
- `Atlas` working-chain setup now uses only:
  - `memory_store`
  - `core_memory_update`
  and returns a clean assistant reply.
- The follow-up recall turn answers directly with no bootstrap-file reads.
- Cross-session isolation also passes:
  - `Atlas` remains isolated in its session
  - `Borealis` remains isolated in its session
  - no cross-contamination was observed in the final assistant replies.

### 10. Asian-Language Runtime Retest
Status: mixed pass
Notes:
- CJK task-state storage/recall works end-to-end:
  - `项目 青龙`
  - `feature/支付修复`
  - `登录回调循环`
  were stored and recalled correctly.
- The UTF-8 trimming crash in Surreal core-memory append path is fixed; no new boundary issues were observed.
- A new durable preference update also completed successfully:
  - `default_city = Tokyo (東京)`
- Remaining bug:
  - a fresh weather/time turn still picked `Berlin` instead of `Tokyo`, even though a direct follow-up recall question correctly answered `Tokyo`.
  - This is no longer a prompt-replay bug; it is now a planner/runtime preference-application bug.

### 11. Matrix Delivery Retest After Compaction
Status: pass with planner caveat
Notes:
- The agent successfully delivered `SYNAPSECLAW MATRIX COMPACT CHECK 2026-04-08T17:20Z`.
- Final response included the correct Matrix room id and event id.
- The planner path is still too noisy:
  - `user_profile(get)`
  - `glob_search`
  - `file_read(send_matrix_test_report.py)`
  - blocked `shell` attempt
  - then recovered via `file_write + shell`
- This confirms Matrix delivery itself is healthy, but configured delivery targets are still not surfaced strongly enough to avoid workspace archaeology.

## Updated Summary

- The original live blocker was real: `chat.send` completion was being held open by synchronous post-response bookkeeping.
- After moving the success-tail to background work, the previously failing everyday flows now complete reliably.
- Duplicate live tool events are no longer reproduced in the current regression pack.
- Provider-facing prompt replay is now materially smaller on ordinary memory turns:
  - no `SOUL.md`
  - no `USER.md`
  - no `AGENTS.md`
  - no `TOOLS.md`
  - no `MEMORY.md`
  reads were observed in the clean `Atlas` / `Borealis` / `青龙` working-chain scenarios.
- The current backlog is now narrower and more specific:
  - planner overuse of workspace archaeology on some delivery/external-action turns
  - weather/time planner not consistently applying `default_city`
  - overly complex shell plans for simple ops checks
  - remaining retrieval contamination / memory hygiene on some local tasks
