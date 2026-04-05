# Self-Learning Algorithms Notes

Source artifact:

- `/tmp/compass_artifact.md`

## Why It Matters

This is the strongest imported note for the next phase after 4.8.

It focuses on:

- selective memory formation
- update vs merge vs delete decisions
- forgetting / decay
- pressure management
- conflict handling
- dual-path learning

That maps directly onto the real remaining gap:

SynapseClaw is getting better at recall and resolution, but still needs a much
stronger learning loop.

## Most Useful Ideas

### 1. Candidate-Based Memory Mutation

Useful direction:

- form typed candidates from runtime evidence
- compare against nearby existing memories
- decide add/update/delete/noop

This aligns with the intended 4.9 candidate pipeline.

### 2. Memory Decay and Quality Control

Useful direction:

- recency decay
- importance weighting
- access frequency
- domain-specific half-lives

This is directly relevant for memory quality, retention, and compaction.

### 3. Memory Pressure Management

Useful direction:

- summarize old context
- preserve raw recall history
- never treat all context as equally important

This fits both prompt-context pressure and memory compaction work.

### 4. Hot-Path vs Background Learning

Very useful distinction:

- hot-path for explicit user corrections / strong signals
- background for broader extraction and reflection

This should shape both self-learning and skill evolution.

### 5. Conflict Resolution

Useful direction:

- provenance-aware conflict handling
- confidence-aware invalidation
- contradiction handling without silent corruption

## What To Keep

- selective learning, not “save everything”
- typed mutation policies
- compaction as first-class behavior
- explicit separation of durable, episodic, and procedural memory

## What To Treat Carefully

- exact formulas and thresholds should not be copied blindly
- original material mixes concepts from several systems; SynapseClaw still
  needs its own contracts

## Best Fit

This note is a direct input into [ipc-phase4_9-plan.md](../ipc-phase4_9-plan.md).
