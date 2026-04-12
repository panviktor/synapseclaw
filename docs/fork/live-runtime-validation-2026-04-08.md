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
- The harmless dynamic weather-location preference-setting turn originally crashed the main daemon because core-memory trimming sliced UTF-8 by byte index in `surrealdb_adapter.rs`.
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

See [tool-fact-porting-instructions.md](../../docs/fork/tool-fact-porting-instructions.md) for the detailed porting guide and acceptance criteria.

## Follow-Up Validation After Runtime And Tool Fixes

Applied during the same validation pass:

- moved `chat.send` success bookkeeping off the synchronous WebSocket completion path
- deduplicated live/persisted tool events by structural signature
- hardened first-batch tool facts without semantic string heuristics
- fixed `user_profile` empty-clear handling; current contract uses dynamic `clear_keys`
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
- The earlier `user_profile` empty-clear failure is gone; current contract uses dynamic `clear_keys`.
- The follow-up weather turn returns the correct weather-city fact (`Berlin`) with no duplicate tool events.
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
- A durable weather-location preference update completed successfully:
  - dynamic profile fact for `Tokyo (東京)`
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

## Follow-Up Validation After 4.10 Typed Turn Defaults

Applied after the next 4.10 slice:

- added typed turn defaults as a shared runtime layer for web/agent paths
- stopped treating configured runtime delivery targets as automatic delivery intent
- restored full web tool visibility while keeping deterministic delivery/default resolution
- fixed `message_send` so `target = null` is treated like an omitted target

### 12. Cheap-Model Route Switching
Status: pass
Notes:
- `/model cheap` switched the live session to `openrouter / qwen/qwen3.6-plus`.
- The follow-up turn returned exactly `CHEAP_OK`.
- `/model gpt-5.4` switched the same live session back to `openai-codex / gpt-5.4`.
- The next turn returned exactly `CORE_OK`.
- Dialog continuity survived the model switch with no visible context loss.

### 13. Dynamic Weather-City Retest
Status: pass with fetch-policy caveat
Notes:
- Setting a dynamic weather-location fact for `Tokyo (東京)` completed successfully through `user_profile`.
- The follow-up weather/time turn resolved `Tokyo`, not `Berlin`.
- The turn fetched local time via `shell` and eventually returned:
  - `Tokyo: ☁️ +16°C`
  - `2026-04-09 06:10 JST (+0900)`
- The remaining issue is no longer default-resolution:
  - `web_fetch` rejected `wttr.in` content type
  - `http_request` rejected `wttr.in` because it is outside the allowlist
  - the turn recovered via `shell(curl ...)`
- This means the old `Berlin` preference-application bug is fixed; what remains is a cleaner weather-fetch path.

### 14. Configured Delivery Target Retest
Status: pass
Notes:
- The live turn `Send the exact text ... to the configured Matrix target.` now emits a single clean `message_send` call with a resolved Matrix target.
- The earlier bad first attempt with `target = null` did not reproduce after the fix.
- The agent sent:
  - `SYNAPSECLAW DELIVERY DEFAULT CHECK 2026-04-08T21:11Z`
- Final response correctly confirmed the configured Matrix target was used.

### 15. Strict Canonical Tool Protocol Retest
Status: pass with one provider-flake caveat
Notes:
- Shared runtime now accepts only:
  - native structured tool calls
  - exact `<tool_call>{json}</tool_call>` fallback envelopes
- Shared runtime no longer tolerates:
  - GLM shorthand
  - perl / `TOOL_CALL`
  - MiniMax XML `<invoke><parameter ...>`
  - JSON alias argument shapes like `parameters`
  - JSON alias ids like `call_id` / `tool_call_id`
- Cheap-route delivery smoke passed cleanly:
  - `qwen/qwen3.6-plus` emitted canonical `message_send({"content":"STRICT CANONICAL CHECK 2026-04-09"})`
  - Matrix delivery completed successfully
- Main-route tool smoke passed cleanly:
  - `gpt-5.4` used the shell tool and returned `STRICT_OK`
- Cheap-route profile mutation was mixed:
  - `user_profile({"action":"upsert","facts":{...}})` executed successfully for the dynamic weather-location fact
  - one upstream OpenRouter response-body decode error interrupted the same turn before final assistant text
  - the follow-up recall turn still answered `Tokyo`
- Interpretation:
  - strict protocol did not regress normal native/canonical tool paths
  - remaining failure there is provider transport instability, not shared-runtime fallback dependence

## Cheap-Route Regression Pack

Default regression lane after this point:

- use the live gateway harness with the cheap route primed first
- reserve explicit `--route gpt-5.4` runs for OpenAI-specific continuation/history experiments

### 15. Cheap Route Smoke
Status: pass
Notes:
- The harness now primes `/model cheap` by default.
- A simple `Reply with exactly HELLO.` turn on the cheap lane returned exactly `HELLO`.

### 16. Cheap Route Memory Isolation
Status: pass
Notes:
- `Atlas` working-chain store and recall both passed on the cheap route.
- Final answer preserved:
  - `release/hotfix-17`
  - `https://staging.atlas.local`
  - `логин через SSO`
- No cross-session contamination appeared in the final answer.

### 17. Cheap Route Service Check
Status: pass
Notes:
- The cheap route executed:
  - `shell(systemctl --user is-active synapseclaw.service)`
- Tool result was `active`.
- Final assistant answer was exactly `active`.

### 18. Cheap Route CJK Memory
Status: pass
Notes:
- The cheap route correctly stored and recalled:
  - `青龙`
  - `feature/支付修复`
  - `登录回调循环`
- This confirms the cheap lane is not regressing non-Latin task-state handling.

### 19. Cheap Route Delivery Default
Status: pass
Notes:
- The cheap route successfully sent:
  - `SYNAPSECLAW QWEN DEFAULT CHECK 2026-04-08T21:18Z`
- The tool call still serialized `target: null`, but `message_send` now resolves that through typed defaults and sends successfully.
- Final assistant reply correctly confirmed delivery to the configured Matrix target.

## Follow-Up Validation After Context Budgeting And Non-Mutating Recall

Applied after the next 4.10 slice:

- added domain-owned provider-context budget assessment
- surfaced budget tier / turn shape / target ceiling in `agent.provider_context`
- added a typed `prefer_answer_from_resolved_state` path so direct structured recall turns can reply with zero tools

### 20. Provider Context Budget Telemetry
Status: pass
Notes:
- Live `agent.provider_context` logs now include:
  - `context_turn_shape`
  - `context_budget_tier`
  - `context_target_total_chars`
  - `context_ceiling_total_chars`
- A clean recall turn on the cheap lane reported:
  - `context_turn_shape = baseline`
  - `context_budget_tier = healthy`
  - `total_chars = 5183`
  - `target_total_chars = 3500`
  - `ceiling_total_chars = 5500`
- This gives us enforceable prompt-economy telemetry instead of only raw char counts.

### 21. Non-Mutating Structured Recall
Status: pass
Notes:
- On `qwen-memory-414`, the store turn updated the working chain normally.
- The follow-up recall turn answered directly from resolved state with:
  - `tool_specs = 0`
  - no `core_memory_update`
  - no `memory_recall`
  - no workspace archaeology
- Final answer correctly returned:
  - `Atlas`
  - `release/hotfix-18`
  - `https://preprod.atlas.local`
  - `миграция session cookies`
- This confirms the new typed non-mutating path is working on live recall turns.

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
- Typed defaults are now materially better in live use:
  - the dynamic weather-location fact for `Tokyo` is applied correctly
  - configured Matrix delivery targets resolve cleanly
  - runtime `/model` switching works without losing session continuity
- The cheap route (`qwen/qwen3.6-plus`) is now viable for the default regression lane:
  - ordinary recall
  - service checks
  - CJK task-state
  - configured Matrix delivery
  all passed in live runs
- Provider-context size is now both smaller and explicitly budgeted.
- Structured recall turns can now reply with zero tools when the runtime already has enough typed state.
- The current backlog is now narrower and more specific:
  - planner still overuses workspace archaeology on some delivery/external-action turns
  - weather fetching still falls back awkwardly because direct fetch tools are constrained for `wttr.in`
  - some recall/info turns still over-mutate in cases that are not yet covered by the new structured-recall path
  - overly complex shell plans for simple ops checks
  - remaining retrieval contamination / memory hygiene on some local tasks

## Follow-Up Validation After Live History Compaction

Applied after the next 4.10 slice:

- introduced live history auto-compaction with summary-generator support
- preserved the latest compaction summary in provider-facing context
- hardened gateway history-delta reconstruction so compaction shrink no longer panics `ws`

### 22. Cheap Route Long-Context Compaction
Status: pass
Notes:
- Session `qwen-compact-419` stored `28` plain-dialogue facts on the cheap lane.
- Final recall correctly returned both early and late facts:
  - `Fact 1 -> release/1, cache-1, City-1`
  - `Fact 2 -> release/2, cache-2, City-2`
  - `Fact 27 -> release/27, cache-27, City-27`
  - `Fact 28 -> release/28, cache-28, City-28`
- The live daemon compacted history in the middle of the run:
  - `history_len_before = 54`
  - `history_len_after = 25`
- The old gateway panic from shrinking history did not recur; `ws` reconstructed the delta safely.
- The next provider-facing snapshot showed:
  - `prior_chat_messages = 7`
  - `prior_chat_chars = 1350`
  which confirms that the compaction summary was surfaced into the provider context.

### 23. Cheap Route Working-Chain Recall After Compaction
Status: pass with wording caveat
Notes:
- `Atlas` and `Borealis` working-chain setup + recall still pass after live compaction landed.
- No cross-session contamination appeared in the final recall answers.
- A minor quality issue remains:
  - the cheap route sometimes answers a pure recall turn with wording like `State updated.`
  - no tool call was observed on that turn, so this is answer phrasing noise rather than a real mutation.

### 24. Cheap Route Mutation Compatibility: Core Memory / User Profile
Status: partial pass
Notes:
- `core_memory_update` now accepts legacy structured shapes emitted by weaker models:
  - direct `key/value`
  - `updates[]` with `key/value`
- The native dispatcher now recovers OpenAI-style nested `<tool_call>` envelopes with:
  - `function.name`
  - `function.arguments`
- After that fix, a cheap-lane preference turn completed successfully:
  - dynamic weather-location fact for `Tokyo`
  - dynamic response-format fact for brief / bullets
  - `ops -> Matrix`, `marketing -> Draft/Delegate`
- The follow-up weather turn again resolved `Tokyo` correctly.

### 25. Cheap Route External Delivery Still Weak
Status: fail
Notes:
- A configured Matrix delivery turn on the cheap lane still did not execute the real delivery tool.
- Instead, the model returned a fenced JSON pseudo-action:
  - `tool = matrix_send_message`
  - `parameters.room_id = ...`
  - `parameters.body = ...`
- No real `message_send` tool call appeared in the daemon logs for that session.
- This is now the clearest remaining cheap-lane gap:
  - info/recall turns are solid
  - local memory/profile mutations are getting better
  - externally meaningful action turns still need a stronger deterministic runtime path.

## Follow-Up Validation After Cheap-Model Condensation Lane

Applied after the next 4.10 slice:

- centralized summary-lane resolution for:
  - agent history compaction
  - web session summaries
  - channel summaries
- precedence is now:
  - explicit `[summary]`
  - `summary_model`
  - `cheap` route
  - current route
- kept the compaction path cheap-first without re-expanding provider-facing context

### 26. Long Semantic Dialogue On Cheap Lane
Status: pass
Notes:
- A long philosophical session on `qwen/qwen3.6-plus` triggered real live compaction:
  - `history_len_before = 54`
  - `history_len_after = 25`
- The long run did not create obvious new procedural skills:
  - repeated `list_skills` reads stayed at `61`
- The run did still create semantic memory anchors when explicitly prompted:
  - early philosophical anchors were stored via `memory_store`
- After tightening the resolved-state safe lane, late memory turns no longer degraded into raw pseudo-tool prose:
  - the follow-up late-anchor turn executed a real `memory_store`
  - the compare turn executed real `memory_recall` calls

### 27. Post-Compaction Recall Hygiene Hardening
Status: pass with residual memory-hygiene caveat
Applied after the next quality pass:

- reranked `memory_recall` results to prefer:
  - core/session anchors
  - over `daily` / `precedent` noise
- added no-progress loop suppression using typed tool-fact progress signatures
- tightened pure-dialogue entity extraction:
  - generic concept-to-concept relations now require extremely high confidence

Live result on `qwen-long-semantic-421`:
- the late-anchor compare turn performed only one `memory_recall`
- the prior repeated recall loop did not reproduce
- provider-facing context remained bounded:
  - `prior_chat_messages = 6`
  - `total_chars` stayed roughly in the `8.0k-9.1k` band on the heavy tail
- the final answer correctly compared the early and late anchors

Residual caveat:
- concept-heavy anchor turns can still produce semantic graph noise
  - example: concept entities around a remembered philosophy anchor
- this is now narrower than before:
  - not a loop problem
  - not a compaction problem
  - mainly a remaining extraction-quality problem

## Follow-Up Validation After Slice 7 Progressive Scoped Loading

Applied after the next 4.10 slice:

- added a scoped-instruction context port and nearest-scope filesystem loader
- wired scoped loading into both web/gateway and channel runtime paths
- injected a dedicated `[scoped-context]` provider block only when structural
  path hints or typed recent workspace/resource context justified it
- kept the decision structural:
  - explicit path-like hints
  - recent typed resource/search/workspace context
  - no extension whitelist in domain logic

### 28. Progressive Scoped Loading
Status: pass with cheap-route caveat
Notes:
- A live subtree test under `~/.synapseclaw/workspace/tmp_scoped_slice7/...` loaded
  real scoped context into the provider-facing snapshot:
  - `scoped_context_chars = 295`
- A direct `gpt-5.4` prompt using that subtree returned the exact scoped phrase:
  - `SUBTREE_SCOPE_CONFIRMED`
- This confirms the main thing Slice 7 needed to prove:
  - scoped instructions are discovered progressively
  - nearest-scope context is injected when a relevant path is present
  - scope loading is no longer tied to eager bootstrap
- Cheap-route caveat:
  - an earlier ambiguous Qwen phrasing still answered as if scoped instructions
    were inactive
  - daemon logs for the same family of turns already showed non-zero
    `scoped_context_chars`, so the remaining weakness is model use of loaded scope,
    not loader/wiring failure

### 29. Cheap Delivery Regression After Slice 7
Status: pass
Notes:
- The cheap route still delivered a configured Matrix message cleanly:
  - `SLICE7_CHEAP_DELIVERY_CHECK_2026-04-09`
- The turn emitted one canonical `message_send({"content":"..."})` call.
- This confirms Slice 7 did not regress the stricter deterministic delivery path.

### 30. Main Route Shell Regression After Slice 7
Status: pass
Notes:
- Main `gpt-5.4` route still executes canonical shell calls correctly.
- Live smoke with `pwd` returned the workspace path:
  - `~/.synapseclaw/workspace`
- An earlier `printf ... > /tmp/...` attempt was blocked by security policy,
  not by tool-routing or prompt-economy regressions.

### 31. Slice-Close Long Semantic Dialogue
Status: partial pass
Notes:
- Cheap-route long dialogue completed all `20` turns without runtime collapse.
- Provider-facing context stayed bounded after compaction:
  - earlier heavy-tail passes during this phase had already reduced post-compaction
    turns into the `8k-9k` band
  - this slice-close run remained in the same general compacted regime instead of
    re-growing linearly
- Procedural-skill pollution did not reproduce:
  - repeated `list_skills` reads stayed at `64`
  - no evidence of new operational run recipes being promoted from the philosophical run
- The remaining failure is semantic, not mechanical:
  - the late anchor was retained correctly:
    - `joy is not proof of truth, but it can be evidence of alignment`
  - the final recall/compression turns replaced the original early anchor
    (`meaning needs both freedom and responsibility`)
    with a later generational anchor
    (`meaning is not inherited; it must be rebuilt by each generation`)
- Interpretation:
  - Slice 7 did not regress compaction
  - Slice 7 did not reintroduce runaway prompt growth
  - the next quality frontier remains long-dialogue semantic ranking / anchor retention

### 32. Slice 8 OpenAI Continuation Probe
Status: partial / backend-blocked
Notes:
- Adapter-local continuation scaffolding was added to `openai_codex`:
  - response ids are tracked in the provider adapter
  - delta-input assembly is ready for user-tail and tool-output follow-ups
  - shared runtime was left unchanged
- Live probe against the deployed `chatgpt.com/backend-api/codex/responses` backend showed:
  - `store=true` is rejected with `Store must be set to false`
  - `previous_response_id` is rejected as an unsupported parameter
- Result:
  - this backend is not currently a continuation-capable OpenAI Responses surface
  - the code now keeps continuation capability gated/disabled by default
  - route behavior was restored and revalidated after the probe
- Post-fallback smoke:
  - `codex-cont-text-003`: pass
    - first turn stored the continuity anchor
    - second turn recalled it correctly
  - `codex-cont-tool-003`: pass
    - tool turn completed and returned `CONT_TOOL_OK`
  - adapter logs showed `continuation_mode=\"disabled\"` on the restored route
    rather than re-sending unsupported parameters

## Refined Summary

- Live compaction is now real, not just planned:
  - it shrinks long session history
  - provider context stays within budget after shrink
  - gateway no longer crashes when history length drops mid-turn
- Cheap-lane `qwen/qwen3.6-plus` is now credible for:
  - recall
  - service checks
  - CJK memory
  - route switching
  - long-context recall after compaction
  - some structured profile/core-memory mutations
- The next real 4.10 blocker is no longer generic prompt size.
- It is the gap between:
  - cheap-model intent expression
  - and deterministic execution of meaningful local/external actions
- The most important remaining target is therefore:
  - universal typed handling for common mutation / delivery intents
  - without reintroducing broad unsafe text-fallback execution.
- Cheap-model condensation itself is now in place across runtime surfaces.
- Progressive scoped loading is also now live:
  - scope docs are injected only when structurally relevant
  - nearest-scope context no longer depends on eager bootstrap
- The new quality frontier after this slice is:
  - long-dialogue semantic anchor ranking
  - residual concept-heavy extraction noise from semantic anchor turns
  - tighter graph hygiene for pure dialogue
  - capability-based routing
  - and provider-native continuation only on endpoints that genuinely support it

## Slice 10 Groundwork

Status: partial, now live-backed rather than plan-only

- Runtime config now has first-class lane-candidate scaffolding:
  - ordered candidates per capability lane
  - manual candidate profile overrides
  - best-effort auto profile enrichment from cached provider catalogs
- The first automatic profile source now comes from cached OpenRouter model metadata:
  - `context_length`
  - `top_provider.max_completion_tokens`
  - modality / supported-parameter derived features
- Web route switching now performs a target-aware preflight when the target
  candidate has a known context window:
  - it evaluates current provider-facing context against the target window
  - it may compact first
  - if the target window is still too small, the switch is refused before the
    provider call
- Live smoke after deployment still passed:
  - `/model cheap` -> `qwen/qwen3.6-plus`
  - `Reply with exactly CHEAP_OK.` -> `CHEAP_OK`
  - `/model gpt-5.4` -> `openai-codex / gpt-5.4`
  - `Reply with exactly MAIN_OK.` -> `MAIN_OK`
- This confirms the route-switch path survived the new candidate/profile layer
  and did not regress ordinary model switching.

## DeepSeek And Phase 4.10 Live-Pack Follow-Up

Status: code-backed / broader pack added

- Direct DeepSeek API probing showed official model ids:
  - `deepseek-chat`
  - `deepseek-reasoner`
- The local direct DeepSeek route was aligned to those ids rather than a
  provider-unknown `deepseek-v4` direct id.
- OpenClaw cross-check:
  - its bundled DeepSeek provider is OpenAI-compatible
  - provider id is `deepseek`
  - base URL is `https://api.deepseek.com`
  - bundled direct models are `deepseek-chat` and `deepseek-reasoner`
  - I did not find a special direct `deepseek-v4` route in its provider plugin
- Live smoke already passed on:
  - Codex / `gpt-5.4`
  - OpenRouter cheap / Qwen
  - direct DeepSeek / `deepseek-chat`
  for HELLO, shell tool, working-chain recall, and CJK recall.
- Known quality signal:
  - direct `deepseek-chat` answered a working-chain recall correctly but still
    emitted an unnecessary `core_memory_update` on the recall turn in one run
  - treat this as a non-mutating-recall quality issue, not a basic provider failure

New mandatory phase-close harness:

```bash
dev/gateway-chat-harness/scripts/phase4_10_live_pack.sh
```

The pack records:

- per-case gateway JSON reports
- provider-context budget rows extracted from systemd journal logs
- admission intent/action signals for media and normal turns
- embedding store/failure/reindex signals
- compaction and summary-lane signals
- optional expensive long semantic dialogue with `RUN_HEAVY=1`

The first run of the new pack completed with no hard failures, but it surfaced
a real context-economy warning:

- provider-context rows: `30`
- max provider-facing context: `8331` chars / `2083` estimated tokens
- over-budget rows: `27 / 30`
- primary ballast sources:
  - `runtime_interpretation`
  - `scoped_context`

This is a quality target for the next context-pressure pass, not a provider
integration failure.

Follow-up on 2026-04-10 after OpenRouter/DeepSeek hardening:

- OpenClaw cross-check still showed no special direct `deepseek-v4` provider
  path:
  - direct provider id: `deepseek`
  - base URL: `https://api.deepseek.com`
  - direct models: `deepseek-chat`, `deepseek-reasoner`
  - context windows in the bundled OpenClaw plugin: `131072`
- Direct `doctor models` passed from the installed binary:
  - `deepseek`: `2` cached models
  - `openrouter`: `350` cached models
- Full default live-pack completed with:
  - failures: `0`
  - warnings: `6`
  - provider-context rows: `32`
  - max provider-facing context: `9390` chars
  - over-budget rows: `29 / 32`
  - embedding signal: pass, repeated `memory.embedding.stored dims=4096`
  - compaction signal: absent in the short pack, expected without `RUN_HEAVY=1`
- Direct `deepseek-chat` still passes correctness on recall/CJK, but one recall
  turn still emitted an unnecessary `core_memory_update`; keep this as a
  non-mutating recall quality bug for the DeepSeek lane.
- OpenRouter vision path no longer fails with the upstream Alibaba error
  `System message must be at the beginning` after adapter-side system-message
  normalization.
- Focused vision smoke on `qwen/qwen3.6-plus`:
  - admitted as `multimodal_understanding`
  - provider-facing `tool_specs = 0`
  - final events had `tool_calls = 0`
  - the model correctly answered `White` for the default 16x16 white PNG

The expensive long-dialogue path should only be run at slice-close points.
