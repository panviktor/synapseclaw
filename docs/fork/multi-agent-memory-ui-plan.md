# Multi-Agent Memory UI / Workbench Plan

## Context

SynapseClaw already has the foundations for a strong operator UI:

- a broker-centered multi-agent dashboard in [`ipc-phase3_8-plan.md`](ipc-phase3_8-plan.md)
- an active memory-learning backend foundation effort in [`memory-learning-foundation-plan.md`](memory-learning-foundation-plan.md)
- durable chat sessions and live events in `web/src/pages/AgentChat.tsx`
- a dedicated memory surface in `web/src/pages/Memory.tsx`
- a warm dashboard/theme system in `web/src/index.css`
- an active memory-unification effort in [`memory-unification-plan.md`](memory-unification-plan.md)

But the current frontend still presents memory and agents mostly as separate admin surfaces. For the operator, the more compelling product is not "a table of memories" but a visible intelligence layer:

- which agent is active right now
- what that agent remembered for this turn
- what it learned from the answer
- how different agents in the fleet are behaving
- how much context budget is being spent

This plan defines a UI direction that keeps the existing one-shell architecture, adds a stronger multi-agent workbench, and gives memory/self-learning a visible "wow" factor without turning the product into a noisy toy.

---

## Goals

1. Make multi-agent operation feel first-class, not hidden behind dropdowns and deep pages.
2. Make memory and self-learning visible in the chat flow, not buried in a storage table.
3. Preserve a clean hexagonal contract: UI reads from explicit gateway/view-model endpoints and events, never from raw storage assumptions.
4. Keep settings small and understandable: presets first, advanced controls second.
5. Add motion and delight without compromising clarity, performance, or operator trust.

## Non-goals

- Building a second separate "broker app" and "agent app"
- Showing raw internal chain-of-thought or hidden reasoning
- Turning every screen into a graph-heavy visualization
- Exposing low-level memory tables as the primary UX
- Adding dozens of memory knobs to the main UI

---

## Design Principles

### 1. One shell, two scopes

The existing direction from [`ipc-phase3_8-plan.md`](ipc-phase3_8-plan.md) stays correct:

- **Fleet scope** for global health, activity, topology, and oversight
- **Agent workbench scope** for chat, memory, tools, logs, and config of one selected agent

The shell must always show which scope the operator is in:

- `Fleet`
- `Agent: opus`
- `Agent: research`

### 2. Memory belongs near conversation

Memory UX should live primarily inside the chat workbench:

- what was used
- what was learned
- what changed

The full Memory page remains useful, but as a studio and inspector, not as the main place where memory becomes understandable.

### 3. Delight follows meaning

Animation should emphasize real state change:

- agent selection
- active run
- memory recall used
- new lesson stored
- skill created
- prompt updated

No decorative motion that says nothing.

### 4. Presets before knobs

Default users should see simple memory behavior presets:

- `Lean`
- `Balanced`
- `Deep`

Advanced users can open a secondary panel for detailed policy and budget controls.

---

## Information Architecture

## Global shell

### Top scope bar

A new top-level scope bar should sit above the page body:

- left: current scope label and breadcrumb
- center: **Agent Rail**
- right: quick actions (`New Chat`, `Open Fleet`, `Search`, `Settings`)

This should become the primary way to move between agents.

### Agent Rail

The Agent Rail is the main multi-agent "wow" surface.

It should be a horizontally scrollable tab bar with compact live cards:

- avatar or monogram
- agent label / role
- live status ring
- trust badge / role badge
- unread activity dot
- tiny memory-heat or activity sparkline
- optional channel badge

Each card should feel alive but controlled:

- subtle pulsing ring for active runs
- soft glow on selected agent
- shared-element highlight when switching agents
- small motion trail when cards reorder or pin

Behavior:

- click to switch agent workbench
- pin favorite agents
- overflow into a searchable "All Agents" sheet
- keyboard switcher: `Cmd/Ctrl+K`
- mobile: collapses into a segmented selector + sheet

This replaces the current agent `<select>` in [`SessionSidebar.tsx`](../../web/src/components/chat/SessionSidebar.tsx) as the primary agent switcher, while keeping the dropdown as a fallback/mobile control if needed.

---

## Core Screens

## 1. Agent Chat Workbench

**Primary surface**: [`AgentChat.tsx`](../../web/src/pages/AgentChat.tsx)

### Layout

Three-column desktop layout:

- left: sessions
- center: chat transcript
- right: **Memory Pulse**

On narrower screens:

- Memory Pulse becomes a slide-over panel
- Agent Rail remains visible at the top

### Memory Pulse

Memory Pulse is the main user-facing self-learning UI.

Sections:

- `Used This Turn`
- `Learned This Turn`
- `Context Budget`
- `Session Summary`

#### Used This Turn

Show what influenced the answer:

- core blocks used
- recalled memory count
- skills injected
- entities referenced

Each item should be tappable:

- preview card
- source type badge (`core`, `episode`, `skill`, `entity`)
- relevance badge if applicable

#### Learned This Turn

Show what changed after the answer:

- daily memory entry stored
- new long-term memory fact
- reflection created
- skill created or updated
- core block updated by optimizer

This should appear as a compact event stack with positive feedback:

- a soft terracotta shimmer on new items
- a short "learning committed" toast
- diff pill for updated blocks

#### Context Budget

The operator should see that memory is budgeted, not uncontrolled:

- total enrichment budget used
- recall share
- skills share
- entities share
- whether continuation mode was `core_only`, `core_plus_recall`, or `full`

This reduces confusion and increases trust.

### Transcript enhancements

Add subtle inline markers under assistant messages:

- `Used memory`
- `Used skill`
- `Learned something`
- `Prompt updated`

These are not verbose debug blobs. They are compact chips that open the right-side Memory Pulse details.

### New-chat delight

When creating a new chat on another agent:

- selected Agent Rail card expands slightly
- chat canvas cross-fades
- first message area animates in from the new agent card

This gives the feeling of "jumping into another mind" without overdesigning it.

---

## 2. Memory Studio

**Base surface**: [`Memory.tsx`](../../web/src/pages/Memory.tsx)

Current implementation status:

- the older raw memory table has been reworked into `Atlas Memoriae`
- the surface is now organized into `Praefrontalis`, `Hippocampus`,
  `Neocortex`, `Amygdala`, and `Archivum`
- projections already expose working state, recipes, skills, contradictions,
  maintenance, clusters, and review decisions as readable operator surfaces
- `MemoryPulse` in chat now shares the same visual language instead of feeling
  like a separate admin widget

The remaining UI work is polish:

- tighter mobile/layout refinement
- motion/detail consistency across workbench and atlas
- optional extra visualizations for lineage and maintenance cadence
- the main chambers are now:
  - `Praefrontalis`
  - `Hippocampus`
  - `Neocortex`
  - `Amygdala`
  - `Archivum`
- structured review surfaces for skills, recipes, contradictions, and cluster
  actions are already visible in the shipped UI
- the raw archive is still preserved as an operator control surface, not removed

The current table is useful but too storage-centric. Turn it into **Memory Studio** with tabs:

- `Working`
- `Episodes`
- `Skills`
- `Knowledge`
- `Reflections`
- `Optimizations`

### Working

Core blocks editor with:

- current values
- recent diffs
- protected block badges
- manual rollback action

### Episodes

Session-aware timeline:

- grouped by session
- grouped by channel
- source badges (`web`, `telegram`, `matrix`, `ipc`)
- truncation preview with expand

### Skills

Most important "self-improvement" screen.

For each skill:

- name
- lesson/description
- success/fail counters
- version
- last used timestamp
- source reflections

Add a `promote`, `disable`, or `rewrite` action later if needed, but keep initial scope read-first.

### Knowledge

Graph-inspired but still readable:

- entity list first
- related facts side panel
- lightweight node-link preview only when helpful

### Reflections

A chronological feed of lessons:

- what worked
- what failed
- lesson
- triggered by tools or error

### Optimizations

Prompt/self-update history:

- which core block changed
- before/after diff
- why the optimizer changed it

This is the page that turns "the system is self-improving" into a credible operator experience.

---

## 3. Fleet Dashboard

**Base surfaces**:

- [`Dashboard.tsx`](../../web/src/pages/Dashboard.tsx)
- IPC fleet pages from Phase 3.8/3.9

### Fleet hero module

Add a broker-only hero band with:

- total active agents
- agents learning today
- total reflections today
- skills created today
- prompt updates applied today
- current fleet context spend

### Constellation View

Broker mode should have one intentionally "wow" visualization:

- a **Constellation View** or **Activity Sky**
- agents as nodes
- glow intensity reflects activity
- ring intensity reflects current run status
- connecting lines reflect recent IPC traffic or collaboration lanes

This should not replace tables. It should complement them.

Use it as a visual overview:

- who is active
- who is isolated/quarantined
- which sub-team is collaborating
- which agent is currently learning heavily

### Heat overlays

Optional overlays:

- `Traffic`
- `Learning`
- `Errors`
- `Cost`

This gives the operator a compelling fleet overview without inventing fake intelligence visuals.

---

## 4. Session Sidebar Refresh

**Base surface**: [`SessionSidebar.tsx`](../../web/src/components/chat/SessionSidebar.tsx)

Keep the existing sidebar but make sessions feel smarter:

- tiny channel badges stay
- add `memory used` indicator
- add `new learning` sparkle
- add `stuck / active` run strip
- add optional summary-preview on hover

For chat sessions spawned on different agents, the sidebar should visually distinguish:

- local sessions
- channel sessions
- proxied broker sessions

without introducing visual clutter.

---

## Motion & Polish

The current theme in [`web/src/index.css`](../../web/src/index.css) already has warm Anthropic-style colors and baseline animations. Build on that rather than changing the visual language completely.

### Motion primitives

- `160-220ms` for hover and chip transitions
- `220-320ms` for layout/scope changes
- `400-500ms` only for meaningful hero animations

### Good animation candidates

- Agent Rail selection morph
- live status ring pulse
- memory item shimmer when a new lesson lands
- right-panel stagger reveal after a turn finishes
- constellation node glow when an agent becomes active

### Avoid

- infinite floating decorations everywhere
- autoplay graph motion with no state change
- loading spinners where skeletons or optimistic updates work better

### Accessibility

- respect `prefers-reduced-motion`
- all color-coded states need icon/text backup
- keyboard navigation for Agent Rail and session switching

---

## Settings Model

Settings should be split into **Simple** and **Advanced**.

## Simple settings

Per agent:

- `Memory Mode`: `Lean`, `Balanced`, `Deep`
- `Show Memory Trace in Chat`
- `Auto Reflection`
- `Auto Prompt Optimization`

These belong in the chat workbench header or a compact settings drawer.

## Advanced settings

Hide behind an expandable panel in Config or Memory Studio:

- continuation policy
- recall max entries
- recall budget
- skill budget
- entity budget
- session-only episodic recall toggle
- manual approval for self-updates
- protected core blocks

The UI must explain impact:

- cost
- latency
- memory depth
- safety
- retention / forgetting profile

Do not expose raw low-level fields without a human-readable explanation.

---

## Backend / Contract Requirements

This UI should not read directly from raw tables or infer behavior from storage layout. It needs explicit read-models.

### Required server-push events

Extend the current websocket event model in `gateway/ws.rs` with higher-level events such as:

- `agent.presence_updated`
- `turn.context_prepared`
- `turn.learning_applied`
- `memory.skill_created`
- `memory.skill_updated`
- `memory.core_blocks_updated`
- `memory.prompt_optimization_applied`

### Required view-model endpoints

Potential new read models:

- `GET /api/agents/workbench`
- `GET /api/agents/:id/memory/overview`
- `GET /api/agents/:id/memory/skills`
- `GET /api/agents/:id/memory/reflections`
- `GET /api/agents/:id/memory/optimizations`
- `GET /api/fleet/learning`

These should sit above storage and compose the unified learning mechanism from [`memory-unification-plan.md`](memory-unification-plan.md).

The payload semantics for those events/read-models should come from:

- [`memory-learning-foundation-plan.md`](memory-learning-foundation-plan.md) for learning/mutation/retention semantics
- [`memory-unification-plan.md`](memory-unification-plan.md) for turn-context and prompt-assembly semantics

---

## Dependencies

This plan depends on two lower layers reaching stable contracts first.

### Hard dependencies

[`memory-learning-foundation-plan.md`](memory-learning-foundation-plan.md)

Reason:

- the UI needs canonical learning events, not guessed storage diffs
- "learned this turn" must map to real mutation/retention decisions
- private vs shared memory must already have stable semantics

[`memory-unification-plan.md`](memory-unification-plan.md)

Reason:

- chat must receive one unified turn-context contract
- post-turn learning must emit one coherent policy/result
- the UI should not encode today’s fragmented implementation

### Soft dependency

[`ipc-phase3_8-plan.md`](ipc-phase3_8-plan.md)

Reason:

- Agent Rail and fleet-vs-agent scope should reuse the existing single-shell broker model

### Dependency order

`memory-learning-foundation-plan.md` → `memory-unification-plan.md` → `multi-agent-memory-ui-plan.md`

---

## Delivery Plan

## Phase 0 — Contract freeze

Precondition before major UI work:

- learning events from the foundation plan exist
- turn-context contract from the unification plan exists
- the web UI no longer needs to infer memory behavior from fragmented runtime paths

## Phase A — Multi-agent shell polish

Files:

- `web/src/components/layout/*`
- `web/src/components/chat/SessionSidebar.tsx`
- new `web/src/components/agents/AgentRail.tsx`

Deliverables:

- top scope bar
- Agent Rail
- agent quick switcher
- pinned/favorite agents

## Phase B — Chat-native memory UX

Files:

- `web/src/pages/AgentChat.tsx`
- new `web/src/components/chat/MemoryPulse.tsx`
- new `web/src/components/chat/TurnTraceChips.tsx`

Deliverables:

- Used This Turn
- Learned This Turn
- Context Budget
- inline answer chips

Prerequisite:

- stable `turn.context_prepared` and `turn.learning_applied` style events

## Phase C — Memory Studio redesign

Files:

- `web/src/pages/Memory.tsx`
- new `web/src/components/memory/*`

Deliverables:

- tabbed studio
- skills/reflections/optimizations surfaces
- working memory diffs

Status:

- largely implemented through the new `Atlas Memoriae` chamber-based redesign
- remaining work is polish and follow-up operator surfaces, not a greenfield
  redesign anymore

Prerequisite:

- stable read-models for mutations, skills, reflections, optimizations, and namespaces

## Phase D — Fleet wow layer

Files:

- `web/src/pages/Dashboard.tsx`
- new `web/src/pages/ipc/FleetConstellation.tsx`
- optional shared visualization components

Deliverables:

- learning health widgets
- constellation view
- fleet overlays

## Phase E — Settings cleanup

Files:

- `web/src/pages/Config.tsx`
- chat/settings drawer components

Deliverables:

- presets first
- advanced memory policy drawer
- clear cost/latency/safety explanations

---

## Verification

1. `cd web && npm run build` compiles after each phase.
2. Local agent mode and broker mode both expose the same agent workbench shell.
3. Switching agents feels instant and preserves scope clarity.
4. A user can see, from chat alone, what memory was used and what was learned.
5. Those chat-level learning states are driven by backend contracts, not inferred from raw rows.
6. The fleet overview remains readable with many agents, not just 2-3.
7. Motion respects reduced-motion settings and does not cause layout instability.
8. Settings remain understandable without reading documentation.

---

## Success Criteria

The UI is successful when an operator can say:

- "I can jump between agents without losing my place."
- "I can tell why this answer happened."
- "I can see when the agent learned something useful."
- "I can understand the fleet at a glance."
- "I can tune memory behavior without becoming a database admin."

That is the target: not more UI, but a clearer sense that SynapseClaw is a coordinated, learning multi-agent system.
