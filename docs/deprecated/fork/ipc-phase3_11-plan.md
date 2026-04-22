# IPC Phase 3.11: Multi-Blueprint Fleet Topology

Phase 3.10: push loop prevention | **Phase 3.11: multi-blueprint topology** | Phase 3.12: channel session intelligence

---

## Problem

The current fleet view was originally designed for a single broker and a relatively flat set of agents. Once the system grows into multiple blueprints, the graph becomes unreadable again for a different reason:

- too many agents from different blueprints are shown at once
- policy topology and observed traffic become visually mixed
- ephemeral children flood the graph with short-lived nodes
- cross-blueprint relationships drown out the internal structure of each blueprint
- operators lose the ability to answer simple questions:
  - which blueprint owns this agent?
  - which blueprints talk to each other?
  - what is declared topology vs temporary traffic?
  - where do I drill down next?

This phase adds a **hierarchical topology model** so fleet navigation works when the broker manages multiple blueprints.

---

## Scope

### In scope

1. A blueprint-level fleet overview graph
2. Blueprint detail topology for agents inside one blueprint
3. Explicit separation of policy topology vs observed traffic
4. Aggregated cross-blueprint traffic links
5. Ephemeral child suppression/collapse by default
6. Drill-down from fleet -> blueprint -> agent/session/trace
7. Route and data model changes needed to support the hierarchy

### Non-goals

- global graph of every agent from every blueprint by default
- arbitrary multi-parent membership for one agent in many blueprints
- a universal graph database for all relationships
- replacing IPC/activity/trace views
- topology inference from embeddings or semantic clustering
- redesigning the Phase 4.0 modular core

---

## Design thesis

The fleet graph must stop pretending that one graph can explain every level of the system.

The operator needs three different views:

1. **Fleet overview**
   - nodes are blueprints
   - edges are aggregated policy or aggregated traffic
2. **Blueprint topology**
   - nodes are agents inside one blueprint
   - edges are policy links by default, traffic only by toggle
3. **Trace / conversation drill-down**
   - not a graph-first view
   - uses existing IPC/session/spawn/channel traces

In other words:

- fleet graph is for **group structure**
- blueprint graph is for **agent structure**
- trace views are for **real dialogs and runs**

---

## Core rules

### Rule 1: Fleet view is blueprint-level by default

`/ipc/fleet` should show one node per blueprint, not one node per agent.

Each blueprint node should summarize:

- blueprint id/name
- number of agents
- online/offline counts
- trust/role composition summary
- optional warnings (degraded, missing agents, proxy failures)

### Rule 2: Blueprint detail is the first agent-level graph

Clicking a blueprint should open a dedicated blueprint topology view.

That view may show:

- agent nodes
- policy links
- optional traffic overlay
- optional ephemeral children

But the fleet overview should not show all of that by default.

### Rule 3: Policy and traffic stay separate

Both fleet and blueprint views need two distinct modes:

- **Policy Topology**
- **Observed Traffic**

Traffic must never be permanently mixed into the default topology layout.

### Rule 4: Ephemeral children are hidden by default

Ephemeral children must not appear as first-class fleet nodes in the overview graph.

At most they should be:

- hidden by default
- collapsed under parent agent in blueprint detail
- visible only behind `Show Ephemeral`

### Rule 5: One agent has one primary blueprint

For v1 topology, each agent belongs to one primary blueprint.

Additional affiliations can exist later as tags or metadata, but not as simultaneous graph parents.

This avoids:

- duplicated nodes
- ambiguous ownership
- unreadable cluster layouts

---

## Data model

### Blueprint

Minimal fields:

```json
{
  "blueprint_id": "research-stack",
  "label": "Research Stack",
  "agent_count": 5,
  "online_count": 4,
  "offline_count": 1,
  "trust_summary": {"1": 1, "2": 2, "3": 2},
  "role_summary": {"coordinator": 1, "worker": 4}
}
```

### Fleet edge

At fleet level, an edge is aggregated between blueprints:

```json
{
  "from_blueprint": "research-stack",
  "to_blueprint": "delivery-stack",
  "type": "traffic",
  "count": 17,
  "window_hours": 24
}
```

Allowed `type` values:

- `policy`
- `traffic`
- `l4_destination` if still useful at blueprint level

### Blueprint detail node

At blueprint level, nodes are agents and reuse the current agent topology fields.

### Ephemeral representation

For v1:

- fleet view: hidden
- blueprint view: hidden by default, optional toggle
- trace views: shown normally where relevant

---

## Views and routes

### 1. Fleet overview

Route:

- `/ipc/fleet`

Default behavior:

- nodes = blueprints
- policy topology by default
- traffic only when toggled

Controls:

- `View: Policy | Traffic`
- `Window: 24h | 7d`
- `Show Cross-Blueprint Links`
- `Show Ephemeral` should be disabled or off by default at fleet level

### 2. Blueprint topology

Route:

- `/ipc/fleet/blueprints/:blueprintId`

Behavior:

- nodes = agents inside blueprint
- policy topology by default
- traffic overlay optional
- hide ephemeral by default

Controls:

- `Show Traffic`
- `Show Ephemeral`
- `Min Traffic Count`
- `Selected Agent Only` (optional)

### 3. Agent workbench drill-down

Routes stay aligned with broker-mode workbench:

- `/agents/:agentId/dashboard`
- `/agents/:agentId/chat`
- `/agents/:agentId/logs`
- `/agents/:agentId/memory`
- etc.

### 4. Trace views

No new graph is required here.

Drill-down should continue to use existing trace surfaces:

- `/ipc/sessions?...`
- `/ipc/spawns?...`
- `/ipc/conversation?...`
- `/agents/:agentId/chat?session=...`

---

## Backend contract

### Fleet overview endpoint

Recommended v1 shape:

- `GET /admin/provisioning/fleet-blueprints`

Returns:

- `blueprints[]`
- `edges[]`

Query params:

- `include_traffic`
- `traffic_hours`
- `traffic_min_count`

### Blueprint detail endpoint

Recommended v1 shape:

- `GET /admin/provisioning/blueprints/:id/topology`

Returns:

- agent-level topology for that blueprint only

Query params:

- `include_traffic`
- `include_ephemeral`
- `traffic_hours`
- `traffic_min_count`

### Membership source of truth

A single persistent source of truth must answer:

- which blueprint each agent belongs to
- which label the blueprint uses
- whether the blueprint is enabled/known

V1 can use provisioning metadata/config if it already exists.

If it does not exist yet, Phase 3.11 must introduce a simple durable registry rather than inferring blueprint membership from historical traffic.

---

## Graph semantics

### Fleet overview graph

Must be **stable and grouped**, not force-chaos.

Recommended layout:

- deterministic cluster or layered layout
- one visual cluster per blueprint node category if needed
- no random initial positions on every refresh

### Blueprint graph

May still use force layout, but:

- policy edges by default
- traffic overlay optional
- message edge width scales with count
- weak links filtered out aggressively

### Traffic defaults

For observed traffic, defaults should be conservative:

- `window = 24h`
- `min_count >= 2`
- hidden unless explicitly enabled

---

## Drill-down behavior

### Fleet -> Blueprint

Click blueprint node:

- open blueprint topology

### Blueprint -> Agent

Click agent node:

- open selected-agent workbench or detail page

### Traffic edge -> Trace

Traffic edges should support opening the next useful layer:

- fleet edge -> filtered blueprint detail or activity feed
- blueprint edge -> filtered activity/IPC sessions between those agents

This is important: operators should not be forced to decode graph lines manually.

---

## Relationship to Phase 3.9

Phase 3.9 solved traceability across IPC/spawn/chat/channel surfaces.

Phase 3.11 does **not** replace that work.

Instead:

- `3.9` answers: “what actually happened?”
- `3.11` answers: “how is the fleet organized at multiple levels?”

`3.11` should always drill down into existing `3.9` trace surfaces instead of inventing a second conversation viewer.

---

## Implementation steps

### Step 1: Blueprint membership model

Introduce or formalize a durable blueprint membership source:

- agent -> blueprint_id
- blueprint label/metadata

### Step 2: Fleet blueprint overview API

Add broker endpoint that returns aggregated blueprint nodes and cross-blueprint edges.

### Step 3: Blueprint detail topology API

Add per-blueprint topology endpoint reusing current agent graph logic with better scoping.

### Step 4: Frontend route split

Add distinct broker UI routes:

- `/ipc/fleet`
- `/ipc/fleet/blueprints/:id`

### Step 5: Fleet-level graph

Render blueprint nodes only.

### Step 6: Blueprint-level graph

Render agent nodes for one blueprint.

### Step 7: Drill-down wiring

Connect blueprint -> agent -> trace navigation.

### Step 8: Traffic/policy toggle behavior

Ensure both levels keep policy and traffic separated.

---

## Verification

### Operator checks

1. With multiple blueprints, `/ipc/fleet` remains readable without hiding half the graph manually.
2. Fleet overview shows blueprint groups, not every agent.
3. Clicking a blueprint reveals only its internal agent topology.
4. Cross-blueprint traffic is aggregated, not exploded into per-agent spaghetti.
5. Ephemeral children do not pollute the default overview.
6. Operators can still drill down from topology into real traces using the existing Phase 3.9 surfaces.

### Failure checks

1. Unknown blueprint membership does not crash the graph.
2. Offline agents do not destroy blueprint grouping.
3. Sparse traffic does not dominate the overview graph.

---

## Risks

1. Blueprint membership may not yet exist as clean metadata.
2. Fleet overview may still become noisy if cross-blueprint traffic is shown by default.
3. Trying to support multi-blueprint membership in v1 would reintroduce graph ambiguity.
4. If drill-down is weak, operators will still fall back to logs and manual hunting.

---

## Decision summary

For multi-blueprint fleets:

- `/ipc/fleet` must become blueprint-level by default
- agent-level topology belongs in blueprint detail view
- traffic must remain optional and filtered
- ephemeral nodes must be hidden/collapsed by default
- traceability must reuse Phase 3.9, not create another disconnected graph
