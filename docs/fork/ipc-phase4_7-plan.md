# Phase 4.7: Deterministic User Context & Task Resolution

Phase 4.6: agent product intelligence | **Phase 4.7: deterministic user context & task resolution** | next: TBD

---

## Problem

Phase 4.6 gives SynapseClaw much better product primitives:

- current-conversation targets
- high-level orchestration tools
- standing orders
- session search
- planner guardrails
- dialogue-state foundations

But the system still fails too often on simple follow-up requests that a strong assistant should resolve without friction.

Typical examples:

| User asks | Current weak behavior | Desired behavior |
|-----------|------------------------|------------------|
| "What's the weather?" | asks which city | uses known default city or recent focus |
| "Translate to my language" | asks which language | uses preferred language |
| "Remind me tomorrow" | asks which timezone | uses known timezone |
| "What did we discuss last week?" | recall may miss | routes to session search / recap |
| "Do it like last time" | weak or inconsistent | uses prior successful run / skill / session recap |

The core issue is not that memory is absent. The issue is that most useful context is still merely **available to the model**, not **resolved by runtime policy**.

Today SynapseClaw can often remember something if the model happens to notice the right context. It still cannot reliably guarantee:

- how user defaults are resolved
- how short references are resolved
- when to search prior sessions
- when to reuse a previous successful execution pattern
- when to ask a clarification, and how narrow that clarification should be

That gap is what still makes simpler systems feel "smarter" in casual use.

---

## Goal

Turn SynapseClaw from "context-rich but heuristic" into "deterministic about common user context and task-resolution patterns."

Specifically:

1. Make stable user defaults first-class structured runtime data.
2. Make short references and follow-up questions resolve through a deterministic policy stack.
3. Route history-oriented and repeat-work questions through explicit resolvers instead of hoping semantic recall fires.
4. Make clarification a last step, not the default first step.
5. Add a measurable eval harness for everyday assistant competence.

---

## Research Basis

This phase is informed by the behavior and public docs of:

- OpenClaw:
  - session-first context model
  - explicit session/search/send primitives
  - strong product-layer handling for recurring operator workflows
- Hermes Agent:
  - high-level tools (`todo`, `clarify`, `send_message`, `session_search`)
  - curated memory and bounded persistent context
  - task scaffolding over raw tool flailing
- Letta:
  - explicit memory blocks for always-on durable context
- LangGraph:
  - clean distinction between thread-scoped state and long-term memory
- Rasa:
  - slot filling and deterministic dialogue-state resolution

Useful references:

- OpenClaw sessions: <https://docs.openclaw.ai/session>
- OpenClaw context: <https://docs.openclaw.ai/context/>
- OpenClaw memory: <https://docs.openclaw.ai/concepts/memory>
- OpenClaw side questions / BTW: <https://docs.openclaw.ai/tools/btw>
- Hermes tools: <https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/>
- Hermes memory: <https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/>
- Hermes sessions: <https://hermes-agent.nousresearch.com/docs/user-guide/sessions/>
- Letta memory blocks: <https://docs.letta.com/guides/core-concepts/memory/memory-blocks/>
- LangGraph memory overview: <https://docs.langchain.com/oss/javascript/langgraph/memory>
- Rasa slots: <https://rasa.com/docs/reference/primitives/slots/>

---

## Diagnosis

What SynapseClaw still lacks relative to the best product behavior of those systems:

1. **Structured user defaults**
   `preferred_language`, `timezone`, `default_city`, and similar fields should not live only as soft text in memory blocks.

2. **Reference resolution**
   Queries like "the second one", "that service", "our chat", "like before", "there", and "tomorrow" need deterministic handling.

3. **History routing**
   Questions about prior work should route to session/runs/skills search first, not rely on ordinary recall.

4. **Repeat-work routing**
   "Do it like last time" needs a path to a previous successful strategy, not just raw episodic memory.

5. **Clarification discipline**
   Asking the user for information the runtime already knows is one of the fastest ways to make the system feel stupid.

---

## Core Principle

The right question is no longer:

> "Can the model infer this from prompt context?"

The right question is:

> "Does the runtime know which resolver to use for this class of request?"

Phase 4.7 turns common assistant behavior from prompt-level guesswork into runtime-level policy.

---

## Resolution Ladder

Introduce a canonical precedence order:

```text
explicit user input
-> dialogue state / working state
-> structured user profile
-> past-work/session resolver
-> long-term semantic memory
-> narrow clarification
```

This order should be enforced by application services, not left entirely to model discretion.

---

## Phase Slices

## Slice 1 — Structured User Profile

### Problem

Stable user facts currently live mostly as soft text in `user_knowledge`-style memory. That helps sometimes, but it does not provide deterministic behavior for things like language or timezone defaults.

### Goal

Add a first-class structured user profile layer for stable defaults and preferences.

### Initial fields

- `preferred_language`
- `timezone`
- `default_city`
- `communication_style`
- `known_environments`
- `default_delivery_target`

### Design

Add a domain model like:

```rust
pub struct UserProfile {
    pub preferred_language: Option<String>,
    pub timezone: Option<String>,
    pub default_city: Option<String>,
    pub communication_style: Option<String>,
    pub known_environments: Vec<String>,
    pub default_delivery_target: Option<String>,
}
```

And a resolver service:

```rust
pub trait UserProfileStorePort {
    async fn load(&self, user_key: &str) -> Result<UserProfile>;
    async fn update(&self, user_key: &str, patch: UserProfilePatch) -> Result<()>;
}
```

### Notes

- This should coexist with core memory, not replace it.
- A sync path can mirror stable profile facts into `user_knowledge`/core blocks for model visibility.
- Runtime resolution should use the structured profile first; prompt enrichment is secondary.

### Acceptance criteria

1. "Translate to my language" resolves through `preferred_language` without asking.
2. "Remind me tomorrow" resolves through `timezone` without asking.
3. "What's the weather?" can use `default_city` when no stronger signal exists.

---

## Slice 2 — Dialogue & Reference Resolver

### Problem

Many follow-up questions are not long-term memory problems. They are short-horizon reference-resolution problems.

Examples:

- "the second one"
- "that service"
- "there"
- "in our chat"
- "what about tomorrow?"
- "and in the other city?"

### Goal

Add a deterministic resolver over session-scoped working state and recent tool subjects.

### Design

Expand `DialogueState` / `WorkingState` to track:

- active topic
- focus entities
- comparison set
- last actionable subject
- last delivery target
- last tool subjects/results
- unresolved ambiguity

Introduce a service like:

```rust
pub struct ResolutionDecision {
    pub resolved_subjects: Vec<ResolvedReference>,
    pub needs_clarification: bool,
    pub clarification_question: Option<String>,
}
```

### Behavioral rule

Short follow-up questions should resolve against:

1. current turn explicit entities
2. dialogue state / comparison set
3. structured user profile
4. session search / past work
5. only then clarification

### Acceptance criteria

1. After discussing two cities, "What is the weather?" no longer falls straight into generic clarify.
2. "The second one" resolves to the right item when a recent comparison set exists.
3. "Send it to our chat" resolves to the current conversation target without archaeology.

---

## Slice 3 — Past Work Resolver

### Problem

`session_search` exists, but the runtime still lacks a first-class rule for historical questions and repeat-work questions.

### Goal

Add a resolver that can distinguish:

- "What did we discuss last week?"
- "What did we decide?"
- "Do it like last time"
- "Use the same approach as before"

### Design

Add a `PastWorkResolver` that can query:

- session search
- prior run summaries
- skill memory
- recent successful recipes

It should return a structured result:

```rust
pub enum PastWorkResolution {
    SessionRecap(SessionRecap),
    RunRecipe(RunRecipe),
    SkillHint(SkillHint),
    None,
}
```

### Behavioral rule

- history questions should route to `session_search` / recap first
- repeat-work questions should route to prior successful run / recipe first
- only then fall back to generic recall or clarification

### Acceptance criteria

1. "What did we discuss last week?" routes to session recap instead of relying on generic recall.
2. "Do it like last time" has a dedicated resolution path.
3. The system can explain whether it used a prior session, recipe, or skill.

---

## Slice 4 — Intent-Driven Resolution Router

### Problem

The runtime still leaves too much to model discretion when deciding which subsystem should answer a request.

### Goal

Introduce a deterministic router from user intent to resolution strategy.

### Design

Add application services like:

- `IntentRouter`
- `ResolutionPlan`
- `ResolutionSource`

Example routing:

| Intent class | First resolver |
|--------------|----------------|
| default-user-context question | `UserProfileResolver` |
| short follow-up / reference | `ReferenceResolver` |
| historical question | `PastWorkResolver` / `session_search` |
| repeat-work question | `PastWorkResolver` / recipe memory |
| delivery/scheduling question | `current_conversation` + standing order / cron |

### Acceptance criteria

1. The same class of user query routes to the same subsystem consistently across web and channels.
2. Historical questions stop randomly depending on ordinary semantic recall.
3. Repeat-work flows stop starting from scratch when a prior successful strategy exists.

---

## Slice 5 — Clarification Policy

### Problem

The system still asks generic clarifying questions too early, even when enough context exists to do better.

### Goal

Make clarification narrow, contextual, and a last resort.

### Design

Add a `ClarificationPolicy` with rules:

- do not ask if explicit input or strong defaults are sufficient
- if multiple candidates exist, ask a bounded disambiguation
- prefer domain-specific wording over generic "which X?"

Examples:

- bad: "Which city?"
- better: "Do you mean Berlin or Tbilisi?"
- bad: "Which language?"
- better: "Should I use your default language, Russian?"

### Acceptance criteria

1. Clarification only happens after resolution attempts fail or confidence is too low.
2. Clarifying questions name the candidate set when available.
3. The system asks fewer unnecessary questions on stable-preference tasks.

---

## Slice 6 — Execution Style & Recipe Memory

### Problem

Skill memory is useful, but it is not yet a reliable answer to "do it like last time."

### Goal

Add a reusable execution-style memory for successful task families.

### Design

Track:

- task family
- prior successful tool sequence summary
- key constraints / approvals used
- preferred execution style
- result quality signal

Expose it as:

```rust
pub struct RunRecipe {
    pub task_family: String,
    pub summary: String,
    pub preferred_steps: Vec<String>,
    pub success_count: u32,
}
```

### Acceptance criteria

1. The system can reuse a prior successful pattern for repeated operational tasks.
2. "Do it like last time" does not depend only on luck in generic recall.
3. Recipes complement, rather than replace, skill memory.

---

## Slice 7 — Everyday Intelligence Eval Harness

### Problem

Without a stable eval suite, the project will keep regressing on exactly the assistant behaviors users care about most.

### Goal

Add a deterministic eval harness for user-context and task-resolution competence.

### Golden scenarios

- "What's the weather?"
- "Translate to my language"
- "Remind me tomorrow"
- "Send it to our chat"
- "What did we discuss last week?"
- "Do it like last time"
- "The second one"
- "Restart that service"
- "Is it still failing?"

### Each eval should record

- which resolver was selected
- whether clarification was necessary
- whether the clarification was narrow or generic
- whether a stable default was used
- whether a prior session/run/skill was reused

### Acceptance criteria

1. Regressions in everyday assistant competence become visible in CI/dev validation.
2. The team can compare SynapseClaw behavior against OpenClaw/Hermes-style expectations using repeatable scenarios.

---

## Architecture Fit

This phase fits the existing hexagonal architecture cleanly.

### Domain / application

Add or extend:

- `UserProfileResolver`
- `ReferenceResolver`
- `PastWorkResolver`
- `IntentRouter`
- `ClarificationPolicy`
- `RunRecipeResolver`
- `ResolutionDecision`

### Ports

Potential new ports:

- `UserProfileStorePort`
- `RunRecipeStorePort`
- optional `PastWorkStorePort`

### Adapters

Implement:

- profile storage / read model
- recipe storage / indexing
- transcript/session lookup adapters
- sync between structured profile and visible core memory

### Important constraint

These decisions should not live as per-channel hacks or prompt-only heuristics. Resolution policy belongs in application services and should be shared by web and channels.

---

## Non-goals

- Replacing the current memory architecture again
- Turning every inference into hard-coded business logic
- Replacing semantic recall entirely
- Building a giant profile/preferences UI in this phase
- Solving every domain with hand-written slots at once

---

## Execution Order

Recommended order:

1. Structured user profile
2. Reference resolver / dialogue-state resolution
3. Past-work resolver
4. Intent-driven router
5. Clarification policy
6. Execution recipe memory
7. Eval harness

This order front-loads the highest user-visible gains.

---

## PR Structure

Suggested PR breakdown:

1. `phase4_7a`: structured user profile + store + sync hooks
2. `phase4_7b`: dialogue/reference resolver
3. `phase4_7c`: past-work resolver + session/runs routing
4. `phase4_7d`: intent router + clarification policy
5. `phase4_7e`: recipe memory
6. `phase4_7f`: eval harness + docs

---

## Success Criteria

This phase is successful when:

1. The assistant stops asking for information it already knows in common everyday flows.
2. Historical and repeat-work questions route to dedicated subsystems instead of generic recall.
3. Clarification becomes rarer, narrower, and more obviously justified.
4. Web and channels share the same user-context resolution policy.
5. SynapseClaw behaves more deterministically than OpenClaw/Hermes on the common scenarios that matter most in everyday use.

---

## Expected Outcome

After Phase 4.7, SynapseClaw should no longer feel like a system that merely *has memory*.

It should feel like a system that:

- knows the user's defaults
- remembers what the conversation is about
- knows when to search prior work
- knows when to reuse a proven approach
- and only asks clarifying questions when it truly has to

That is the next major step from "powerful runtime" to "actually smarter assistant."
