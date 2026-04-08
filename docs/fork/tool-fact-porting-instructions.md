# Tool Fact Porting Instructions

Purpose:
- make tool-heavy runtime turns produce useful typed facts for Phase 4.8 / 4.9
- reduce planner drift into bootstrap/workspace archaeology
- give a smaller/cheaper model enough structure to finish the next tool-porting batch safely

This document is intentionally operational, not aspirational.
It is based on live validation from [live-runtime-validation-2026-04-08.md](/home/protosik00/synapseclaw/docs/fork/live-runtime-validation-2026-04-08.md), not on guessed gaps.

## Current Baseline

Coverage proxy:

| Metric | Count | Notes |
|---|---:|---|
| `crates/adapters/tools/src` source files | 64 | rough tool-adapter scope proxy |
| files with `extract_facts(...)` or `execute_with_facts(...)` | 25 | partially or fully migrated |
| files still without typed-fact hooks | 39 | likely still legacy/minimal |

Observed tools from live validation:

| Tool | Observed calls | Current state | Priority |
|---|---:|---|---|
| `content_search` | 14 | partial | P1 |
| `file_read` | 13 | partial | P1 |
| `shell` | 11 | partial | P1 |
| `memory_recall` | 5 | partial | P1 |
| `glob_search` | 4 | partial | P1 |
| `core_memory_update` | 3 | partial | P1 |
| `user_profile` | 2 | good baseline | reference |
| `session_search` | 1 | good baseline | reference |
| `precedent_search` | 1 | good baseline | reference |
| `file_write` | 1 | good baseline | lower priority |
| `file_edit` | 1 | good baseline | lower priority |

## First Batch

Port these first, in this order:

1. `memory_recall`
2. `core_memory_update`
3. `shell`
4. `file_read`
5. `content_search`
6. `glob_search`

Reference implementations:

- [user_profile.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/user_profile.rs)
- [session_search.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/session_search.rs)
- [precedent_search.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/precedent_search.rs)
- [file_write.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/file_write.rs)
- [file_edit.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/file_edit.rs)

## Porting Rules

Every migrated tool should satisfy all of the following:

1. Keep the existing user-visible output stable unless the current output is clearly broken.
2. Emit typed facts through `execute_with_facts(...)` or `extract_facts(...)`.
3. Emit at least one semantic fact beyond the generic outcome fact when the tool succeeds.
4. Emit no long-lived semantic facts on failed calls unless the failure itself is meaningful.
5. Avoid turning bootstrap/test scaffolding into durable memory.
6. Add focused unit tests for the new fact output.

Do not stop at “it already has some facts”.
For this batch, “ported” means the facts are rich enough to improve:

- retrieval ranking
- instruction turns
- profile learning
- service-check workflows
- contamination control

## Tool-Specific Targets

### 1. `memory_recall`

File:
- [memory_recall.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/memory_recall.rs)

Current problem:
- emits only a generic `FocusFact` over top entries
- loses the difference between “found a user preference”, “found a delivery target”, “found a workflow precedent”, and “found random memory noise”

Target behavior:
- keep the current focus entities
- add a real search-style fact for the recall operation
- preserve top categories / top locators so downstream retrieval can tell what kind of recall happened

Preferred typed output:
- `ToolFactPayload::Search(...)` with:
  - a dedicated memory-oriented domain if the existing enum cannot represent recall cleanly
  - the original query
  - result count
  - primary locator from the top hit
- `ToolFactPayload::Focus(...)` for the top 1-3 hits

Important:
- if adding a new search domain is necessary, do it narrowly and document it in the enum comment
- do not silently alias generic memory recall to `Knowledge` unless the match really is graph knowledge

Tests to add:
- successful recall emits search + focus facts
- empty recall emits no semantic fact
- recall of mixed categories keeps a stable primary locator choice

### 2. `core_memory_update`

File:
- [core_memory_update.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/core_memory_update.rs)

Current problem:
- current fact only says “core_memory_block X was appended/replaced”
- loses whether the update was:
  - user preference
  - task state
  - delivery target
  - workspace context
  - domain workflow note

Target behavior:
- preserve the generic core-block fact
- add typed facts derived from the block label and content when it is safe to do so

Priority mappings:
- `user_knowledge`
  - emit `UserProfileFact` when content clearly encodes:
    - preferred language
    - timezone
    - default city
    - communication style
    - default delivery target
- `task_state`
  - emit `FocusFact`
  - emit `WorkspaceFact` / `ResourceFact` / `DeliveryFact` when content clearly contains those anchors
- `domain`
  - emit a workflow-oriented focus fact only if the content actually describes reusable procedure

Important:
- do not build a giant natural-language parser here
- start with obvious, high-confidence patterns
- prefer missing a fact over inventing one

Tests to add:
- user preference update emits `UserProfileFact`
- task-state update with workspace/delivery info emits typed task-state facts
- invalid block/action still emits no facts

### 3. `shell`

File:
- [shell.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/shell.rs)

Current problem:
- current facts are basically “cwd” + raw command focus
- that is too weak for service diagnostics, update checks, and runtime learning

Target behavior:
- preserve current focus fact
- add structured hints for common command families used in live ops turns

First command families to recognize:
- `systemctl` / `systemctl --user`
  - service target
  - action kind: inspect / status / restart / start / stop
- `journalctl`
  - service/log target
  - inspection intent
- `curl .../health`
  - endpoint/resource verification
- `apt update`
  - update refresh action
- `apt list --upgradable`
  - update inventory/search result
- `df`, `free`, `uptime`
  - system-state observation

Preferred typed output:
- `ResourceFact` for services, endpoints, files, repositories where appropriate
- `SearchFact` for package/update inventory queries where appropriate
- `FocusFact` for residual command context
- outcome fact remains automatic

Important:
- do not try to parse arbitrary shell syntax perfectly
- handle the common “obvious ops” patterns first
- keep fallbacks generic for everything else

Tests to add:
- `systemctl --user is-active synapseclaw.service` emits a service-oriented fact
- `journalctl -u synapseclaw.service` emits an inspection-oriented fact
- `apt list --upgradable` emits an update/search-oriented fact

### 4. `file_read`

File:
- [file_read.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/file_read.rs)

Current problem:
- only emits a generic file resource read fact
- live validation showed excessive reads of `SOUL.md`, `USER.md`, session archives, and other bootstrap artifacts

Target behavior:
- keep the resource fact
- add small metadata that helps retrieval distinguish bootstrap reads from task reads

Preferred typed output:
- `ResourceFact` with:
  - path
  - read operation
  - byte count when available
- optionally add `FocusFact` for the basename/path category only when it is not a known bootstrap artifact

Bootstrap/noise rule:
- reads of files like `SOUL.md`, `USER.md`, session archives, and other agent bootstrap scaffolding should not become strong semantic facts
- they may still produce a resource fact, but should be tagged/handled as low-signal in tests and downstream ranking

Tests to add:
- normal task file read emits a resource fact
- bootstrap file read remains low-signal and does not emit task-like focus facts

### 5. `content_search`

File:
- [content_search.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/content_search.rs)

Current problem:
- it already emits a search fact, but live traces show it dominating archaeology loops
- current facts are not rich enough to distinguish:
  - productive task search
  - bootstrap churn
  - “searching because the planner is lost”

Target behavior:
- preserve current search fact
- enrich it with better locators and stable query metadata

Preferred typed output:
- `SearchFact` with:
  - workspace domain
  - query
  - result count
  - primary locator
- optionally add `FocusFact` for the top 1-2 matched paths when those paths are task-relevant and not bootstrap noise

Important:
- do not emit dozens of path facts
- cap semantic facts aggressively

Tests to add:
- productive search emits a stable primary locator
- zero-match search emits no semantic facts
- bootstrap-only matches do not produce strong task focus

### 6. `glob_search`

File:
- [glob_search.rs](/home/protosik00/synapseclaw/crates/adapters/tools/src/glob_search.rs)

Current problem:
- current search fact is better than nothing, but still too shallow for ranking/bootstrap suppression

Target behavior:
- keep the workspace search fact
- enrich locators and distinguish low-signal bootstrap globs from task globs

Preferred typed output:
- `SearchFact` with:
  - workspace domain
  - original pattern
  - result count
  - primary locator
- optionally one `FocusFact` for the first relevant matched path when it is not a bootstrap artifact

Tests to add:
- normal glob emits a search fact with first match
- empty glob emits no facts
- bootstrap-only glob does not become a strong task signal

## Validation Workflow

For each tool change:

1. implement the fact logic
2. add or update unit tests in the same file
3. run targeted checks

Suggested commands:

```bash
cargo test -q -p synapse_tools <tool_module_or_test_name>
cargo check -q -p synapse_tools -p synapse_adapters -p synapse_domain
```

After a small batch:

```bash
cargo build --release --features channel-matrix
```

Then redeploy and re-run the live harness scenarios.

## Acceptance Criteria

A tool migration is good enough when:

- the tool emits typed facts that materially improve downstream recall/learning
- the live harness trace shows fewer pointless follow-up searches for the same intent
- the tool does not create new durable noise from bootstrap/test scaffolding
- existing user-visible tool output remains correct

## What Not To Do

- Do not invent giant NLP parsers inside tools.
- Do not add speculative payload variants unless the current enums truly cannot express the signal.
- Do not emit huge fact lists from one tool call.
- Do not turn bootstrap files or test disclaimers into durable memory.
- Do not “port” a tool by only adding another generic focus fact with no new semantics.
