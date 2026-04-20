# 4.11 Slice 5 Plan

Document name: 4.11 Slice 5 Plan

## Scope

Slice 5 turns skills into a governed runtime subsystem, not just prompt text.
The system must handle two skill lanes:

- source-authored skills: skills we write, install, import, or ship as packages
- runtime-generated skills: skills learned or improved from repeated successful
  procedures, repair traces, and operator feedback while the program runs

The goal is not to add another prompt-bloat mechanism. The runtime should expose
a compact skill catalog, activate only relevant skill content, dedupe activated
skills, and keep full skill/package history outside provider context unless the
model actually needs it.

## Research Summary

### Hermes Agent

Hermes is the closest reference implementation for our target shape. It stores
skills as `SKILL.md` packages under `~/.hermes/skills/`, supports bundled,
hub-installed, external, and agent-created skills, and uses progressive
disclosure: list compact metadata first, load full content only through
`skill_view` when needed.

The important Hermes idea for us is the `skill_manage` loop: after a complex
successful task, a corrected workflow, or a discovered non-trivial procedure,
the agent can create or patch a skill. Hermes also runs agent-created and
community skills through a trust-aware guard, validates frontmatter, limits
content size, supports category folders, and keeps supporting files in
`references/`, `templates/`, `scripts/`, and `assets/`.

What Hermes does not solve enough for us is typed runtime governance. It has
availability/setup/tool/platform checks, but Slice 5 needs a domain-owned
resolver that returns explicit states such as `active`, `candidate`, `shadowed`,
`disabled`, `incompatible`, `blocked_missing_capability`, `needs_setup`, and
`deprecated` across web and channel surfaces.

Local evidence reviewed:

- `<local-hermes-agent-checkout>/tools/skills_tool.py`
- `<local-hermes-agent-checkout>/tools/skill_manager_tool.py`
- `<local-hermes-agent-checkout>/tools/skills_guard.py`
- `<local-hermes-agent-checkout>/tools/skills_hub.py`
- `<local-hermes-agent-checkout>/website/docs/user-guide/features/skills.md`
- `<local-hermes-agent-checkout>/website/docs/developer-guide/creating-skills.md`

### OpenHands Skills

OpenHands now treats skills as specialized prompts with always-on, trigger
loaded, and progressive-disclosure modes. Project skills take precedence over
user-level skills, and AgentSkills packages can expose `name`, `description`,
and `location` while the model reads full content on demand.

The useful part for us is precedence and loading strategy. The weak part is
conflict handling: when multiple skills share a trigger, OpenHands can load all
of them and concatenate content. For SynapseClaw we should not concatenate
conflicting instructions. We should resolve shadowing and blocked states before
anything reaches provider context.

### AgentSkills / Vercel Skills

AgentSkills formalizes the same package contract: a compact catalog with name,
description, and location; then file-read or dedicated activation for full
content. The integration guide explicitly recommends hiding disabled or
permission-denied skills from the catalog and tracking activated skills to avoid
duplicate injection.

This maps directly to our token-budget requirement. Our default should become a
governed compact catalog plus activation, not full skill body injection.

### Voyager

Voyager is the reference for real self-improving procedural skills. It builds an
ever-growing library of executable code skills, retrieves relevant skills,
improves programs from environment feedback and execution errors, and verifies
skills through self-checking before reuse.

The lesson is that learned skills must be evidence-backed and executable or
procedural enough to compound. For us, this means learned skills should come
from `RunRecipe` clusters, tool traces, and verified outcomes, not from a single
LLM reflection. It also means every promoted skill needs a verification story:
what task it helps with, what tools it uses, and how we know the procedure still
works.

### LangMem / LangGraph

LangMem separates semantic, episodic, and procedural memory. Its procedural
memory path updates behavior from scored trajectories and feedback, often as
prompt updates or instruction proposals.

For SynapseClaw, the useful pattern is proposal generation, not blind adoption.
An LLM may draft a `SkillDraft` or `SkillPatchCandidate`, but deterministic
gates, evidence thresholds, contradiction checks, security checks, and operator
review decide whether it becomes active.

### AutoGPT

AutoGPT's block system is less like skill learning and more like modular
workflow composition. Blocks are typed workflow components that users compose
into automations.

The useful part is not self-learning, but typed capability surfaces. A skill
that needs a route, tool, lane, model modality, or external setup should declare
that contract so the runtime can block it before the model tries to use it.

### Mem0 and A-MEM

Mem0 and A-MEM are memory systems, not skill systems. Mem0 is useful as a
reference for async memory, metadata filtering, graph relationships, reranking,
and extraction policy. A-MEM is useful for dynamic memory organization,
automatic tagging, linking, and evolution of related memories.

For Slice 5, they inform discovery and evolution of learned skills: retrieval
can use embeddings and graph links to find candidates, but activation state must
still be deterministic and capability-aware.

### Security and Evaluation Findings

Recent public work on Agent Skills shows that skill packages are a realistic
prompt-injection and supply-chain surface. We should assume community,
agent-created, and generated skills can contain malicious or over-broad
instructions.

SkillsBench also reports an important caution: curated skills can improve pass
rates, but self-generated skills are not reliably helpful on average. This is a
strong argument for generated skills entering as candidates with tests and
operator review, not as automatically active prompt instructions.

## Current SynapseClaw State

Already present:

- file/package skills in `crates/adapters/core/src/skills/mod.rs`
- workspace skills and opt-in open-skills loading
- skill security audit for installed package directories
- full vs compact prompt injection mode
- CLI `skills list/install/audit/remove`
- domain `Skill`, `SkillOrigin`, `SkillStatus`, and `SkillMemoryPort`
- learned skill promotion from repeated `RunRecipe` success thresholds
- learned skill review and contradiction/failure-cluster gates
- active learned skill retrieval in turn context

Current gaps:

- package skills and memory skills are not one registry
- full prompt injection remains the config default
- runtime skill state is not a first-class domain decision
- no single resolver considers agent, channel/platform, category, route,
  model/tool capabilities, setup, disabled policy, and shadowing together
- blocked skills are not surfaced with typed reasons in diagnostics
- learned skills can be created/refreshed, but there is no package-level
  candidate/review workflow for generated skill documents and patches
- skill activation is not deduped/protected as a structured context artifact
  across compaction
- web/channel behavior is not governed through one shared skill resolver

## Target Architecture

### 1. Skill Registry

Add a domain-facing registry that merges all skill sources into one normalized
view:

- `Manual`: operator-authored workspace skill packages
- `Bundled`: skills shipped with the product
- `Imported`: open-skills, AgentSkills, GitHub, hub, or migration imports
- `External`: read-only shared directories
- `Learned`: skills generated from recipe evidence
- `GeneratedPatch`: candidate improvements to an existing skill

The registry should preserve source path, content hash, parent skill id,
origin, trust level, package metadata, and evidence ids. It should not copy full
package content into memory unless the skill is learned-only and has no package
file yet.

### 2. Skill Manifest Contract

Extend parsed skill metadata into a typed manifest. Required minimal fields:

- `name`
- `description`
- `origin`
- `status`
- `source_ref`
- `content_hash`

Optional governance fields:

- `category`
- `task_family`
- `triggers`
- `platforms`
- `channels`
- `agents`
- `required_tools`
- `required_tool_roles`
- `required_toolsets`
- `fallback_for_tools`
- `fallback_for_toolsets`
- `required_model_lanes`
- `required_modalities`
- `required_environment_variables`
- `required_config`
- `disable_model_invocation`
- `review_required`
- `trust_level`

Triggers must be hints only. They can improve retrieval, but the resolver cannot
depend on English keyword matching. Skill state and capability blocking must be
language-independent and metadata-driven.

### 3. Skill Governance Resolver

Add a domain service:

`SkillGovernanceService::resolve(input) -> SkillResolutionReport`

Input:

- agent id
- session id
- channel/platform
- user-visible surface: CLI, web, Matrix, Telegram, etc.
- task text and optional task family
- route profile and model capabilities
- available tool registry and tool roles
- configured skill policy
- registry candidates
- prompt budget
- current activated skill ids

Output per skill:

- `state`: `active`, `candidate`, `shadowed`, `disabled`, `incompatible`,
  `blocked_missing_capability`, `needs_setup`, or `deprecated`
- `reason_code`
- `shadowed_by`
- `missing_capabilities`
- `setup_requirements`
- `source_ref`
- `activation_mode`: `catalog_only`, `load_body`, `already_loaded`,
  `operator_review_required`
- `prompt_projection`: bounded metadata only

Resolution order:

1. reject deprecated or disabled skills
2. reject channel/platform/agent incompatible skills
3. reject unavailable tool/model/modality capabilities
4. mark missing secret/config setup as `needs_setup`
5. apply source precedence: manual/bundled > imported/external > learned
6. apply shadowing by name, task family, and tool pattern overlap
7. apply learned-skill promotion and contradiction gates
8. rank relevant active skills using task family, metadata, embeddings if
   available, and prior success evidence
9. emit only bounded active/catalog entries to provider context

Embeddings are useful for candidate search and ranking, but not required for
deterministic state. If the embedding lane is unavailable, resolver behavior
must degrade to metadata, task-family, and recency/evidence ranking.

### 4. Runtime Activation

Replace full eager skill body injection with a governed activation path:

- compact catalog in provider context by default
- dedicated `activate_skill`/`read_skill` adapter path, or allowlisted file-read
  path where safe
- max active skill count and max active skill chars per turn
- session-level activated skill set for dedupe
- structured skill content wrapper for compaction and diagnostics
- no re-injection of a skill already active in the provider-visible session
- activated skill content is protected or summarized during compaction instead
  of duplicated

This keeps the provider prompt small and makes skill activation observable.

### 4A. On-Demand Skill Activation Model

The default skill behavior must match the Codex-style on-demand model, not the
Claude-style startup preload model. At turn start the provider sees only a
bounded catalog entry for each eligible skill: name, short description, source,
state, location/activation id, and selected capability hints. The full `SKILL.md`
body is not loaded until the task actually needs it.

Activation has two paths:

1. runtime pre-activation when the resolver has high confidence from explicit
   user command, task family, channel command, or deterministic metadata match
2. model-requested activation when the model chooses a skill from the compact
   catalog and calls `activate_skill` / reads the skill location

Both paths go through the same resolver. A skill that is disabled, shadowed,
missing capabilities, missing setup, or still a candidate is not exposed as an
active loadable instruction. It can appear in diagnostics, but not as provider
behavioral guidance.

Activated skill bodies are session-scoped structured context artifacts. The
runtime records which skill version is active, avoids duplicate injection, and
preserves the identity plus short summary across compaction instead of replaying
the same body repeatedly.

### 4B. Skill Loading Algorithm

Skill loading is a per-turn admission pipeline. It should be deterministic where
possible and model-assisted only for relevance/ranking, never for policy.

Per-turn algorithm:

1. build a `SkillLoadRequest` from the current turn: agent id, session id,
   channel/platform, user text, explicit slash command or mention, current route
   profile, available tools/tool roles, model capabilities, prompt budget, and
   already activated skill versions
2. collect candidates from the registry without loading bodies: explicit skill
   id/name, active package skills, active learned skills, recent candidates for
   diagnostics, task-family matches, declared trigger hints, required capability
   matches, and optional embedding/semantic matches when the embedding lane is
   available
3. run `SkillGovernanceService` over the candidate set: disabled/deprecated,
   platform/channel incompatibility, missing tool/model capability, missing
   setup, trust policy, review policy, and shadowing are resolved before any
   skill can be loaded
4. split decisions into three views:
   - provider catalog: bounded metadata for loadable active skills only
   - runtime preloads: high-confidence skills that should be loaded before the
     model answers
   - diagnostics: candidate/blocked/shadowed/setup-needed skills, never injected
     as behavioral instructions
5. rank loadable skills: explicit user activation first, then exact task-family
   match, then required tool/capability match, then historical success for this
   agent/channel, then semantic score, then recency; manual/bundled/imported
   skills outrank learned skills on overlap
6. choose activation mode:
   - `preload`: explicit command or deterministic high-confidence match; load
     the body before provider call
   - `catalog_only`: relevant but not certain; expose compact entry and let the
     model request activation
   - `blocked`: do not expose as loadable; show reason only in diagnostics
   - `already_loaded`: do not inject again; provide only active skill identity
7. enforce budget: cap number of catalog entries, cap preloaded bodies, cap chars
   per skill, and prefer summaries/resources pointers over large bodies; if the
   budget is tight, drop low-score catalog entries before dropping active task
   facts
8. when the model requests activation, re-run governance on the exact skill id
   and current route/tool state, then load the body only if still active and
   compatible
9. wrap loaded content as a structured context artifact with skill id, version,
   source, content hash, activation reason, and resource list; do not eagerly
   read `references/`, `scripts/`, or `assets/`
10. record `SkillActivationTrace`: selected candidates, blocked reasons, loaded
    skill ids, budget cost, route/model, and outcome; use this trace later for
    skill ranking and generated patch candidates

Candidate discovery and model awareness:

- the model does not search the full skill store itself; the runtime discovers
  likely skills before provider call
- candidate discovery uses layered signals: explicit skill command/name,
  task-family metadata, capability/tool requirements, historical skill success
  for this agent/channel, embedding similarity, and graph links between skills,
  recipes, tools, agents, channels, success clusters, and failure clusters
- embedding and graph search operate outside provider context and return only a
  bounded top-k shortlist; they must never dump raw memories, all skill bodies,
  or graph neighborhoods into the prompt
- model awareness is limited to compact catalog cards: skill id/name, short
  description, state, source, activation id/location, and capability hints
- graph/embedding relevance can propose or rank a skill, but cannot override
  policy; only the governance resolver can mark a skill loadable
- MVP discovery should work without embeddings: explicit activation, metadata,
  task family, route/tool capability, and historical evidence are enough for a
  deterministic baseline
- embedding search is the second layer for fuzzy/multilingual task matching;
  graph ranking is the third layer for procedural memory links and repeated
  workflow evidence

Important behavior:

- explicit user command can request a skill, but cannot bypass disabled,
  security, missing capability, or missing setup gates
- candidate skills are visible to operators and reviewers, not to the provider as
  active instructions
- trigger words are hints, not authority; multilingual users should still work
  through task-family, capability, route, and semantic matching
- model-requested activation must use a stable skill id/name from the catalog,
  not arbitrary path guessing
- compaction preserves active skill identity and compact summary; it does not
  duplicate full `SKILL.md` bodies

### 4C. Skill Authoring Algorithm

Generated skills are composed by a pipeline, not by the main chat model deciding
that a memory sounds useful. The main model may supply evidence during normal
work, but a dedicated `SkillEvolutionService` owns the candidate lifecycle.

Algorithm:

1. collect evidence after each turn: `RunRecipe`, successful tool pattern,
   verification result, user correction, repair trace, failed attempts, active
   skill ids, route/model, and channel
2. cluster evidence by task family, tool pattern, objective, and outcome
3. reject weak clusters: too few successes, no verification, contradictory
   failure cluster, already covered by higher-priority manual/imported skill
4. choose output type: new `SkillDraft` when no matching skill exists, or
   `SkillPatchCandidate` when an existing skill was used but a repeated gap was
   discovered
5. draft with a specialist authoring lane: a cheap model can summarize evidence,
   but a stronger model should write or revise the skill when the candidate will
   affect production behavior
6. normalize the draft into strict `SKILL.md` sections: frontmatter, when to
   use, prerequisites, procedure, verification, failure modes, rollback/safety,
   and references/scripts to load only on demand
7. run deterministic validation: manifest schema, capability declarations,
   static security audit, size budget, no hidden prompt-injection patterns, no
   unsupported tool/model claims
8. run replay/eval checks: with-skill vs without-skill harness where possible,
   plus skill-specific smoke commands or verifier assertions
9. store as `candidate` with provenance, diff, tests, and reviewer-facing
   diagnostics
10. promote only after operator approval or an explicit auto-promotion policy
    that requires repeated passing evidence and no unresolved contradictions

The skill author is therefore a bounded subsystem: extractor plus clusterer plus
LLM drafter plus deterministic validator plus review gate. The LLM writes the
proposed text; the domain policy decides whether it becomes active.

### 5. Runtime Generated Skills

Generated skills should be created from evidence, not vibes.

Candidate creation inputs:

- repeated successful `RunRecipe` clusters
- successful tool traces with stable tool pattern and verification result
- user corrections that changed the procedure
- repair traces that converged to a working path
- operator feedback on a previous candidate

Candidate creation gates:

- minimum repetition threshold
- stable task family or stable intent cluster
- non-empty verification signal
- no contradictory failure cluster above threshold
- no higher-priority manual/imported skill shadowing it
- static security scan clean enough for candidate status

Output:

- `SkillDraft` for new learned skill
- `SkillPatchCandidate` for improving an existing skill
- provenance links to recipes, traces, failures, and review decisions
- generated body in AgentSkills-compatible `SKILL.md` shape
- test cases or replay criteria generated alongside the draft

Generated skills start as `candidate`. Promotion to `active` requires either
operator approval or a product policy that allows auto-promotion only after
passing deterministic replay/eval gates.

### 6. Skill Improvement Loop

When a skill is used and the run exposes a gap, create a patch candidate instead
of silently editing the active skill:

1. record `SkillUseTrace`: skill id, task family, route, tools, result,
   verification, failure/repair evidence
2. cluster traces by skill and failure mode
3. draft a minimal patch when repeated evidence supports the change
4. run static audit and manifest validation
5. run skill-specific tests or replay checks
6. place patch in operator review queue
7. after approval, write a new version and keep previous version rollbackable

Manual skills should not be auto-patched without an explicit policy. Learned
skills can be auto-refreshed more aggressively, but still need contradiction and
security gates.

### 7. Security Model

Security checks should run at three depths:

- fast static scan: path traversal, symlinks, shell chaining, high-risk command
  patterns, prompt-injection signatures, oversized files
- manifest/capability scan: declared tools, env vars, network needs, scripts,
  platform requirements, permission scope
- suspicious-skill review: LLM or multi-pass reviewer only for risky packages,
  not on the hot path

Policy by trust:

- bundled/manual: active by default if audit passes
- trusted imported: active or candidate depending on operator policy
- community imported: candidate until audit/review
- generated: candidate until evidence and review gates pass

Dangerous findings must block activation. Caution findings may stay candidate
with a clear reason and operator override path.

### 8. Diagnostics and Operator Review

Add shared diagnostics for web and channels:

- `/skills status`
- `/skills blocked`
- `/skills candidates`
- `/doctor skills`

CLI equivalents:

- `synapseclaw skills status`
- `synapseclaw skills review`
- `synapseclaw skills health`
- `synapseclaw skills promote <id>`
- `synapseclaw skills reject <id>`
- `synapseclaw skills diff <id>`
- `synapseclaw skills test <id>`
- `synapseclaw skills apply <id>`
- `synapseclaw skills versions [skill]`
- `synapseclaw skills rollback <apply-record-or-snapshot>`

Diagnostics must show active and blocked skills without dumping full skill
content. Each blocked item needs a reason such as missing tool, missing model
lane, unsupported platform, disabled policy, setup needed, or shadowed by a
higher-priority skill.

## Implementation Slices

### 5.1 Domain Types

Add domain types for:

- `SkillSource`
- `SkillTrustLevel`
- `SkillRuntimeState`
- `SkillCapabilityRequirement`
- `SkillSetupRequirement`
- `SkillRuntimeDecision`
- `SkillResolutionReport`
- `SkillActivationRecord`
- `SkillDraft`
- `SkillPatchCandidate`

Keep these in `synapse_domain` first. Adapters should only load packages and
execute side effects.

### 5.2 Registry Adapter

Build a registry adapter that reads:

- workspace skill packages
- open-skills/imported packages
- external read-only package dirs
- learned skills from `SkillMemoryPort`

The adapter emits normalized records with metadata, path, content hash, source,
trust level, and optional body pointer. It should not eagerly load all bodies
into provider context.

### 5.3 Governance Resolver

Implement `SkillGovernanceService` with deterministic tests for all Slice 5
states. Start with metadata and capability gates. Add embedding-assisted ranking
only after deterministic state resolution is stable.

### 5.4 Prompt and Context Integration

Move package skills and learned skills behind the same resolver. Make compact
catalog plus activation the default. Preserve full mode only as a legacy
operator setting.

Add activation dedupe and compaction protection for activated skill content.

### 5.5 Learned Skill Candidate Pipeline

Wire existing `RunRecipe` promotion and `SkillReviewService` into the unified
registry. Instead of just writing learned skills into memory, generate a
reviewable `SkillDraft` with provenance and tests.

### 5.6 Skill Patch Pipeline

Add `SkillUseTrace` and `SkillPatchCandidate` from repeated repair/correction
evidence. Generated patches should be small diffs where possible, not full-file
rewrites.

### 5.7 Review and Diagnostics

Expose status, blocked reasons, candidates, diffs, and review actions through
shared web/channel command rendering and CLI commands.

### 5.8 Security and Evaluation

Extend the current skill audit into a layered audit. Add candidate replay/eval
tests and store pass/fail results with the candidate.

## Acceptance Tests

Required Slice 5 tests:

- manual active skill shadows learned skill with same task family
- disabled skill is not active for web or channel
- skill requiring an unavailable tool/model capability is blocked with a clear
  reason
- repeated successful recipe creates or refreshes a learned skill candidate
- contradictory failure cluster blocks promotion

Additional tests needed for this plan:

- imported skill shadows learned skill by name and task family
- generated skill remains `candidate` until review or eval gate passes
- candidate skill is visible in diagnostics but not loaded into provider context
- missing env/config produces `needs_setup`, not generic disabled
- platform/channel mismatch produces `incompatible`
- missing tool role produces `blocked_missing_capability`
- compact catalog omits full skill body and includes only bounded metadata
- activating the same skill twice does not duplicate context
- manual compaction preserves active skill identity without duplicating full
  body
- web and channel commands render the same skill resolution report
- web and channel `/skills tools` render the same runtime replay contract
  inventory used by skill replay/eval
- malicious package with prompt-injection or exfiltration pattern is blocked
- successful repair trace creates a patch candidate, not an active durable skill
- skill benchmark must compare with-skill vs without-skill behavior before
  broad activation
- every runtime tool registry entry passes typed `ToolContract` validation; the
  gate rejects missing schema properties, unsafe replayable required args,
  sensitive replay payloads, invalid transforms, and provider-facing replay
  schema extensions
- replay/eval inventory is generated from the same runtime tool contracts used
  by execution, so docs and diagnostics cannot drift from the executable policy
- learned skill discovery uses the active embedding provider for shortlist/rank
  when available, while governance state remains metadata/capability driven and
  deterministic
- generated patch apply is refused unless replay/eval passed, target version
  still matches, and every typed procedure claim is backed by resolved repair
  provenance; successful apply increments the target skill version and leaves a
  rollback snapshot outside normal provider-facing retrieval

## Success Metrics

- provider-visible skill context stays under a configured budget
- full skill body is loaded only for activated skills
- resolver emits typed state for every considered skill
- generated candidates include provenance and replay/eval criteria
- learned skills improve repeated task success or latency in harness tests
- blocked skills are understandable to operators without reading logs
- no web/channel divergence in skill status or activation behavior

## Non-Goals

- no automatic activation of arbitrary generated scripts
- no loading every installed skill body into every prompt
- no English-only keyword trigger gate
- no replacement of existing `SkillMemoryPort`
- no provider call on the hot path just to decide whether a skill is disabled or
  missing a capability

## Current Implementation Status

Implemented so far:

- domain-owned skill governance resolver with typed runtime states, capability/setup gates, shadowing, activation modes, prompt projections, and budget enforcement
- shadowing now covers duplicate activation ids/names across sources, so imported
  or manual skills can correctly suppress lower-priority learned skills instead
  of both remaining active under the same logical skill identity
- compact skill catalog as the default prompt injection mode, with full mode kept as an operator override
- learned skill retrieval now passes through governance before a skill can enter runtime context
- generated-skill domain contracts for `SkillDraft`, `SkillPatchCandidate`, evidence refs, and activation traces
- CLI diagnostics for package skills: `skills status`, `skills blocked`, `skills candidates`
- CLI operator workflow for learned/generated skills: `skills learned`, `skills review [--apply]`, `skills promote`, `skills demote`, and `skills reject`
- `skills candidates` now includes learned candidates as well as package candidates
- direct learned-skill CLI access fails fast when the embedded memory store is locked by a running daemon instead of silently falling back to noop memory
- daemon/gateway-backed learned-skill API: list learned skills/candidates, dry-run review, apply review, promote/demote/reject learned skills, and strict status updates behind the existing bearer-token API
- CLI learned-skill workflow now falls back to the authenticated gateway when direct memory is unavailable, so the daemon can remain the embedded database owner
- Unicode-aware skill name/status matching replaced the remaining ASCII-only checks in the Slice 5 API/CLI paths
- runtime `skill_read` tool provides governed on-demand skill activation by id/name/location, supports package and learned skills, checks the governance resolver before returning full instructions, and dedupes repeated activation in the same runtime
- `skill_read` now writes compact `SkillActivationTrace` records after loaded,
  already-loaded, or blocked activation attempts; traces preserve selected,
  loaded, and blocked skill ids plus typed block reasons without storing full
  skill bodies
- manual/runtime compaction now reads recent activation traces and adds bounded
  `active_skill_identity` handoff hints, preserving active skill ids across
  compaction without replaying or duplicating the loaded skill body
- compact skill catalog now points the model to `skill_read` instead of raw file-read as the default activation path
- shared web/channel `/skills` rendering is now centralized through the runtime command host and the same skill governance resolver/formatter used by CLI diagnostics
- shared web/channel skill review commands now cover `/skills status`, `/skills blocked`, `/skills candidates`, `/skills review [--apply]`, `/skills promote <id-or-name>`, `/skills demote <id-or-name>`, and `/skills reject <id-or-name>` through the common domain parser and adapter-core executor
- deterministic domain service now converts repeated resolved repair traces into inactive `SkillPatchCandidate` proposals with repair provenance and replay criteria instead of editing active skills directly
- generated `SkillDraft` and `SkillPatchCandidate` records now carry replay/eval results, and a deterministic eval gate reports whether candidates are eligible for operator/policy promotion without promoting them directly
- post-turn learning now receives live bounded tool-repair history, generates patch candidates for overlapping learned skills, persists them into a memory-backed review queue, and exposes queued generated patches through governed `/skills candidates` status
- patch-candidate provenance now carries structured evidence metadata, and the replay/eval service builds typed replay cases with target skill, required tool, provenance ids, and with-skill/without-skill comparison cases instead of parsing human-readable criteria text
- `skills test <patch-candidate>` and `POST /api/skills/candidates/test` now run typed replay/eval cases through the runtime tool registry, persist fresh eval results back into the candidate queue, and keep promotion blocked when executable replay payloads are missing or failed
- patch promotion is now case-aware: generated patch candidates require a
  passed executable repair replay case and a passed with-skill/without-skill
  comparison case, so a candidate cannot pass promotion merely by satisfying an
  unrelated textual criterion
- patch candidates now carry typed procedure claims derived from resolved repair
  provenance, and the with-skill/without-skill comparison validates those claims
  as data instead of scanning markdown bodies for claim substrings
- replay/eval now emits compact `SkillUseTrace` records for the target skill,
  including eval outcome, verification summary, tool pattern, and repair
  evidence refs, so candidate promotion has an auditable use-trace source
  without putting skill bodies into memory trace records
- post-turn learning now emits compact live `SkillUseTrace` records for
  activated learned skills when the current turn actually exercised the skill's
  typed tool pattern; outcome comes from typed repair/evidence state, and the
  trace is memory/audit material rather than provider prompt text
- live `SkillUseTrace` feedback now updates learned-skill success/failure
  counters only after a trace is persisted; failure attribution is per skill tool
  pattern, so unrelated tool failures in the same turn do not penalize the
  activated skill
- replay/eval execution revalidates stored `replay_args` against the selected
  tool's current `ToolContract` immediately before execution, so stale or
  tampered candidates cannot execute non-replayable tools such as shell
- CLI `skills test` now builds the same full runtime tool registry shape as the
  agent/gateway path instead of the old default-only tool subset, so replay
  candidates can exercise `repo_discovery`, `git_operations`, `web_fetch`,
  `workspace`, and `precedent_search` when those tools are required
- successful tool repairs now attach bounded `replay_args` to the repair trace only when the tool exposes structured JSON arguments through a replayable runtime role; `shell.command` is deliberately excluded because free-form command text is not a typed replay contract
- replay payload sanitization is contract-driven: replay args require a typed tool contract, only declared schema properties are retained for shape/type validation, sensitive fields are dropped by contract policy, and payload size/depth limits prevent context or memory bloat
- tool replay policy now has a typed `ToolContract` layer: tools declare replayability, argument allow/block rules, allowed enum values, and transforms such as URL origin+path in Rust instead of relying on provider-facing JSON schema extensions
- read-only workspace probes now declare replay contracts for `file_read`, `glob_search`, `content_search`, `repo_discovery`, `git_operations`, and `web_fetch`; Git replay is limited by typed contract to read operations (`status`, `diff`, `log`, `branch`, `release_status`) and supports typed `repo_path` instead of shell-style `git -C ...` command text
- `git_operations.release_status` provides a typed version/source audit path for Matrix-like local repositories: it resolves an allowed `repo_path`, reads the nearest local tag, reads remote tags with `git ls-remote`, compares semantic tag versions, and returns structured `outdated`/`comparison` fields for replay and skill tests
- `repo_discovery` provides bounded repository discovery over explicitly allowed roots: it scans without shell/find, does not follow symlinks, skips `.git` internals, filters by typed parameters, and returns structured candidate repository paths for follow-up `git_operations.status`/`git_operations.release_status` replay
- `web_fetch` replay payloads now use a typed URL transform that stores only parsed `scheme://host[:port]/path`; credentials, query strings, and fragments are stripped before any replay args can enter repair traces
- adapter-core now exposes a registry-level tool contract audit and inventory:
  default tools and the runtime registry must pass `ToolContract` validation in
  tests before a replayable tool can feed generated skill candidates
- operators can inspect the live replay contract inventory through
  `synapseclaw skills tools` and shared `/skills tools` runtime commands on web
  and channels
- operators can inspect compact persisted skill-use evidence through
  `synapseclaw skills traces`, `GET /api/skills/traces`, and shared
  `/skills traces` runtime commands; the view is bounded and contains ids,
  typed outcomes, tool patterns, verification summaries, and evidence counts
  rather than full skill bodies
- operators can inspect read-only skill catalog health through
  `synapseclaw skills health`, `GET /api/skills/health`, and shared
  `/skills health` runtime commands; the report folds together skill metadata,
  success/failure counters, recent compact use traces, and deterministic review
  decisions into bounded severity/recommendation signals without mutating skill
  lifecycle state
- operator-approved cleanup can apply the eligible learned-skill lifecycle
  subset through `synapseclaw skills health --apply`,
  `POST /api/skills/health/apply`, and shared `/skills health --apply`;
  the cleanup path only changes learned skill status, reuses the existing
  versioned `update_skill` mutation path, and deliberately skips manual or
  imported skills that need metadata fixes rather than lifecycle mutation
- operators can inspect generated patch candidates through
  `synapseclaw skills diff <patch-candidate>`,
  `POST /api/skills/candidates/diff`, and shared `/skills diff <id>` runtime
  commands; the view shows target/version, typed procedure claims,
  replay/eval status, and a bounded line-delta preview without loading the
  patch into provider context
- operators can apply generated patch candidates through
  `synapseclaw skills apply <patch-candidate>`,
  `POST /api/skills/candidates/apply`, and shared `/skills apply <id>` runtime
  commands; apply is blocked unless the candidate is pending, target skill and
  version match, replay/eval gates pass, and typed procedure claims are backed
  by resolved repair provenance
- patch apply now stores a rollback snapshot as a hidden deprecated learned skill
  and a compact apply audit record with candidate id, target version, rollback
  skill id, typed procedure claims, provenance, and eval reason; the full
  previous body stays out of provider-facing context and normal skill retrieval
- operators can inspect and roll back generated patch applications through
  `synapseclaw skills versions [skill]`, `synapseclaw skills rollback <ref>`,
  `GET /api/skills/versions`, `POST /api/skills/rollback`, and shared
  `/skills versions` / `/skills rollback` runtime commands; rollback restores
  the saved snapshot only when the target skill is still at the applied version,
  then writes a compact rollback audit record
- generated patch auto-promotion is now a typed dry-run/apply policy, not a
  hidden background write: `synapseclaw skills autopromote`,
  `GET /api/skills/autopromote`, and shared `/skills autopromote` evaluate
  replay/apply gates plus recent compact live skill-use traces; explicit
  `--apply` writes only when `[skills.auto_promotion].enabled=true`
- the default auto-promotion config is enabled and requires clean live evidence
  before apply: by default a patch needs two recent `Succeeded` live traces for
  the target skill and zero recent `Failed`/`Repaired` traces in the inspected
  window, in addition to passing replay/eval and provenance gates
- learned skills are embedded as compact skill cards (`name`, `description`,
  `task_family`, tools, lineage, tags, bounded procedure excerpt) and
  `find_skills` uses vector search when an embedding profile is active; turn
  retrieval preserves that semantic order before governance filters loadable
  skills
- skill embedding writes are best-effort and profile-consistent: a skill row only
  receives `embedding_profile_id` together with a successful vector, and failed
  refresh clears stale skill vectors instead of leaving outdated semantic search
  material behind
- `skill_read` now has adapter tests proving that active learned skills can be
  loaded from memory on demand while candidate learned skills are refused before
  operator review/promotion
- package skill audit now rejects high-risk prompt-injection and credential
  exfiltration patterns before workspace/open-skills packages can enter the
  runtime skill catalog
- ordinary user-authored skills now have a first-class memory-backed creation
  path: CLI `skills create`, gateway `POST /api/skills/create`, and shared
  runtime `/skills create <name> :: <body>` turn plain markdown into
  `origin=manual` skills with audit, normalization, tags, status, and the same
  embedding/governance retrieval path as learned skills
- operators can list user-authored memory skills through
  `synapseclaw skills authored` and `GET /api/skills/authored`; lifecycle
  status updates accept local manual or learned skills while still refusing
  imported/package skills that are not owned by the memory store
- memory-backed manual/learned skills can now be exported into editable
  file-backed packages through `synapseclaw skills export <id-or-name>` and
  `POST /api/skills/export`; the export writes a normal `SKILL.md` package
  under `workspace/skills`, preserves source id/origin/status/task/tool hints in
  frontmatter, and runs the existing package audit before reporting success
- memory-backed manual/learned skills can now be operator-edited through
  `synapseclaw skills update`, `POST /api/skills/update`, and shared
  `/skills update <skill> :: <body>` runtime commands; edits run inline markdown
  audit, write compact version records, save rollback snapshots outside active
  retrieval, and can be reverted with the same `skills rollback` path as
  generated patch applications
- operators can create safe editable package skeletons with
  `synapseclaw skills scaffold`; the command creates `SKILL.md`, `references/`,
  `templates/`, and `assets/`, then runs the existing package audit before the
  scaffold is considered usable
- provider-facing context now treats `skill_read` bodies as short-lived
  activation material: the immediate tool cycle can expose the body, while older
  completed tool cycles are represented by compact activation receipts with
  id/name/source/body hash instead of repeating full skill instructions
- semantic turn retrieval now formats matched memory-backed skills as compact
  catalog entries with id/name/status/metadata and a `skill_read` activation
  instruction; full skill bodies stay out of turn enrichment and duplicate
  skill refs are collapsed before provider context assembly
- the default skill prompt helper and config documentation now align with the
  runtime default: compact catalog is the default, while full instruction
  inlining is only an explicit legacy/diagnostic mode
- compact `Available Skills` prompt assembly now applies a bounded catalog
  policy: duplicate package skills collapse by activation id, manual packages
  shadow imported/open-skills packages, and catalog entries are capped with an
  explicit omitted-count marker instead of growing unbounded
- compact package catalog selection is source-prioritized before applying the
  cap, so workspace/manual package skills cannot be pushed out of provider
  context merely because imported/open-skills packages loaded first
- workspace package porting now filters file-backed runtime skills by source:
  local manual `SKILL.*` packages are ported into memory and moved to
  `skills/ported/`, while imported/open-skills packages remain visible in the
  compact catalog and can still be activated through `skill_read`
- imported/open-skills packages now get compact memory-backed semantic index
  cards with source refs and content hashes; this lets omitted package skills be
  discovered by `find_skills`/turn retrieval without placing full package bodies
  in provider context, and stale index cards are deprecated when the file-backed
  source is no longer enabled
- skill health now folds typed activation traces, use traces, and rollback
  records into compact utility counters (`selected`, `read`, `helped`,
  `failed`, `repaired`, `blocked`, `rollbacks`) without reading skill bodies or
  parsing natural-language success/failure strings
- user-authored skills have deterministic E2E coverage for create -> governed
  `skill_read` activation -> activation trace -> health utility accounting, so
  normal operator skills use the same on-demand path as generated/imported
  skills
- package porting and file-backed package indexing have deterministic fixtures
  for loose `SKILL.*` files, nested packages, existing imported packages,
  compact index cards, stale index deprecation, and file-body activation through
  `skill_read`

### Slice 5 Closeout Checklist

Status after the 2026-04-18 hardening pass:

| # | Closure item | Status |
| --- | --- | --- |
| 1 | Diff/help smoke for generated patch candidates | Closed: `skills diff <id>` is exposed in CLI/gateway/runtime command flow and live-smoked against `patch-proof-1776484922`. |
| 2 | Review/apply flow for patch candidates | Closed: apply requires passing replay/eval, matching target version, and backed procedure claims; stale patch live-smoke correctly refused promotion. |
| 3 | Rollback/version trail for applied patches | Closed: patch/manual updates create apply records, rollback snapshots, `skills versions`, and rollback audit records. |
| 4 | Separate auto-promotion and operator approval policy | Closed beta: `[skills.auto_promotion]` policy gates automatic apply separately from operator review/apply. |
| 5 | Keep memory/session/project replay private until typed policy exists | Closed for Slice 5: unsafe replay contracts remain excluded; only typed replay-safe tools are enabled. |
| 6 | User-authored skill create/import path | Closed beta: `skills create`, markdown/frontmatter import, audit, memory storage, lifecycle, and `skill_read` activation are implemented. |
| 7 | Manual skill lifecycle | Closed beta: create, update, promote/demote/reject, versions, rollback, export, and scaffold/import paths exist. |
| 8 | Plain markdown/user skill audit | Closed beta: user-authored and package skills pass audit before storage/export/porting. |
| 9 | On-demand loading without prompt bloat | Closed: compact catalog omits bodies; `skill_read` is the only body activation path and dedupes repeated reads. |
| 10 | Document generated patch/new/user/imported modes | Closed in this plan: beta status matrix separates generated patch, generated new, user-authored, imported/open package, and replay tools. |
| 11 | SKILL.md one-time port/ported transition | Closed beta: workspace `SKILL.*` packages are imported to memory and moved under `skills/ported/`; imported/open packages remain file-backed. |
| 12 | Unified web/channel command mechanism | Closed beta: `/skills` status/review/health commands share the same parser, executor, and formatter across web/channel paths. |
| 13 | Context cleanup and utility metrics for skills | Closed beta: activation traces survive compaction as bounded identity hints, health reports expose typed utility metrics, and bodies are not duplicated in provider context. |

### Verification Log

Local deterministic checks:

- `cargo fmt --check`
- `cargo test -q -p synapse_domain skill_` (77 tests)
- `cargo test -q -p synapse_domain parse_skills_health`
- `cargo test -q -p synapse_adapters skills::tests --features channel-matrix` (44 tests)
- `cargo test -q -p synapse_adapters skill_runtime --features channel-matrix` (8 tests)
- `cargo test -q -p synapse_memory skill_semantic_search_tests` (2 tests)
- `cargo check -q -p synapse_domain -p synapse_adapters -p synapseclaw --features channel-matrix`
- `cargo build --release --features channel-matrix`

Live fleet checks:

- installed the release binary to `~/.cargo/bin/synapseclaw`
- restarted `synapseclaw.service` and helper services
  `copywriter`, `marketing-lead`, `news-reader`, `publisher`, and
  `trend-aggregator`
- all six services reported `active`
- health checks on ports `42617` through `42622` returned `status=ok`
- `synapseclaw skills health --limit 5 --trace-limit 20` returned typed
  utility counters and inspected activation/rollback evidence
- `synapseclaw skills health --limit 5 --trace-limit 20 --apply` returned
  "No skill cleanup lifecycle changes are eligible."
- gateway harness replied exactly `HELLO`
- gateway harness `/skills health` rendered the same shared health report
- `synapseclaw skills candidates --limit 3` listed learned candidates and
  generated patch candidates without the old stray "No matching skills" line
- `synapseclaw skills diff patch-proof-1776484922 --limit 5` rendered current
  vs proposed body delta and warned about stale target version
- `synapseclaw skills test patch-proof-1776484922 --limit 5` refused promotion
  because the candidate targets `v1` while the current skill is `v3`

### Slice 5 Beta Status Matrix

| Lane | Current status |
| --- | --- |
| Generated patch | Beta-ready: diff, replay/test, apply, version trail, rollback, and guarded auto-promotion are implemented and live-proven. |
| Generated new skill | Candidate/review path exists through learned skills; needs broader negative E2E fixtures and consolidation policy before production default trust. |
| User-authored skill | Beta-ready: create, update, export, versions, rollback, audit, embedding retrieval, governance, and `skill_read` activation use the memory-backed path. |
| Imported/open package | Beta transition: audited file packages load, workspace `SKILL.md` packages port into memory and move to `ported/`; package editing UX is scaffold/export based. |
| Replay tools | Beta-ready for typed read-only contracts; `memory_recall`, `session_search`, and `project_intel` remain intentionally excluded until privacy classification exists. |

### Replay Tool Inventory

Tools with replay contracts now enabled:

- `file_read`: replayable read-only workspace probe. Payload: `path`,
  optional `offset`, optional `limit`.
- `glob_search`: replayable read-only workspace discovery. Payload: `pattern`.
- `content_search`: replayable read-only workspace content search. Payload:
  declared regex/search options only.
- `repo_discovery`: replayable local repository discovery. Payload: optional
  `root_path`, `max_depth`, `limit`, `name_contains`, and `include_bare`.
- `git_operations`: reference implementation for typed replay policy. Replay is
  limited to `status`, `diff`, `log`, `branch`, and `release_status`; write
  operations are excluded by typed contract.
- `workspace`: replayable runtime-state inspection for `list` and `info`;
  mutating actions (`switch`, `create`, `export`) are excluded by typed contract.
- `precedent_search`: replayable procedural-memory lookup for prior successful
  recipes.
- `web_fetch`: replayable external lookup with URL payload normalized by parser
  to origin plus path only; userinfo, query strings, and fragments are not
  persisted.

Explicitly not replayable:

- `shell`: excluded because free-form command text is not a typed replay
  contract.
- `skill_read`: intentionally omitted because it mutates the runtime activation
  set and can expand large instruction bodies into the tool result.
- `memory_recall` and `session_search`: excluded until stored replay queries
  have a privacy classification and bounded payload policy.
- `project_intel`: excluded because inputs are often large private project
  payloads; replay needs a separate redaction/shape policy first.

### Tool Contract Audit Gate

The replay contract is executable policy, not documentation. Each tool exposes
its `ToolContract` from Rust, and adapter-core validates the full default and
runtime registries against the provider schema before replay/eval can trust the
tool. The audit rejects provider-facing replay markers, undeclared arguments,
replayable secrets, missing non-replayable reasons, replay transforms on
non-string schema fields, and required schema arguments that cannot be replayed.

The same contract source also produces an inventory row for diagnostics and
documentation. This keeps the replayable tool list synchronized with execution
and makes future tools fail tests until they declare whether they are replayable,
why they are not replayable, and which arguments may enter a skill replay case.

Still open for later Slice 5 work:

- add replayable subsets for safe memory/session/project-intel lookups after
  privacy and payload-shaping policies are explicit
- add richer package editing UX beyond the current scaffold/export/import
  baseline, especially guided edits for `references/`, `templates/`, and
  `assets/`

## References

- Hermes Agent local implementation: `<local-hermes-agent-checkout>`
- Hermes Skills System: https://hermes-agent.nousresearch.com/docs/user-guide/features/skills
- OpenHands Skills Overview: https://docs.openhands.dev/overview/skills
- OpenHands SDK Skills Guide: https://docs.openhands.dev/sdk/guides/skill
- AgentSkills integration guide: https://agentskills.io/client-implementation/adding-skills-support
- AgentSkills best practices: https://agentskills.io/skill-creation/best-practices
- Voyager: https://github.com/MineDojo/Voyager
- LangMem SDK announcement: https://www.langchain.com/blog/langmem-sdk-launch
- AutoGPT Blocks: https://agpt.co/docs/integrations
- Mem0 OSS features: https://docs.mem0.ai/open-source/features/overview
- A-MEM: https://github.com/agiresearch/A-mem
- Agent Skills prompt-injection paper: https://arxiv.org/abs/2510.26328
- SkillSieve security triage paper: https://arxiv.org/abs/2604.06550
- SkillsBench evaluation paper: https://arxiv.org/abs/2602.12670
